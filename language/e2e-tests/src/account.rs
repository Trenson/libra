// Copyright (c) The Libra Core Contributors
// SPDX-License-Identifier: Apache-2.0

//! Test infrastructure for modeling Libra accounts.

use crate::{gas_costs, keygen::KeyGen};
use anyhow::{Error, Result};
use libra_crypto::ed25519::*;
use libra_types::{
    access_path::AccessPath,
    account_address::AccountAddress,
    account_config::{
        self, from_currency_code_string, type_tag_for_currency_code, AccountResource,
        BalanceResource, KeyRotationCapabilityResource, ReceivedPaymentEvent, SentPaymentEvent,
        WithdrawCapabilityResource, COIN1_NAME, COIN2_NAME, LBR_NAME,
    },
    event::EventHandle,
    transaction::{
        authenticator::AuthenticationKey, RawTransaction, Script, SignedTransaction,
        TransactionArgument, TransactionPayload,
    },
    write_set::{WriteOp, WriteSet, WriteSetMut},
};
use move_core_types::{
    identifier::{IdentStr, Identifier},
    language_storage::{ResourceKey, StructTag, TypeTag},
    move_resource::MoveResource,
};
use move_vm_types::{
    loaded_data::types::{FatStructType, FatType},
    values::{Struct, Value},
};
use std::{collections::BTreeMap, str::FromStr, time::Duration};
use vm_genesis::GENESIS_KEYPAIR;

// TTL is 86400s. Initial time was set to 0.
pub const DEFAULT_EXPIRATION_TIME: u64 = 40_000;

pub fn lbr_currency_code() -> Identifier {
    from_currency_code_string(LBR_NAME).unwrap()
}

pub fn coin1_currency_code() -> Identifier {
    from_currency_code_string(COIN1_NAME).unwrap()
}

pub fn coin2_currency_code() -> Identifier {
    from_currency_code_string(COIN2_NAME).unwrap()
}

/// Details about a Libra account.
///
/// Tests will typically create a set of `Account` instances to run transactions on. This type
/// encodes the logic to operate on and verify operations on any Libra account.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Account {
    addr: AccountAddress,
    /// The current private key for this account.
    pub privkey: Ed25519PrivateKey,
    /// The current public key for this account.
    pub pubkey: Ed25519PublicKey,
}

impl Account {
    /// Creates a new account in memory.
    ///
    /// The account returned by this constructor is a purely logical entity, meaning that it does
    /// not automatically get added to the Libra store. To add an account to the store, use
    /// [`AccountData`] instances with
    /// [`FakeExecutor::add_account_data`][crate::executor::FakeExecutor::add_account_data].
    /// This function returns distinct values upon every call.
    pub fn new() -> Self {
        let (privkey, pubkey) = KeyGen::from_os_rng().generate_keypair();
        Self::with_keypair(privkey, pubkey)
    }

    /// Creates a new account with the given keypair.
    ///
    /// Like with [`Account::new`], the account returned by this constructor is a purely logical
    /// entity.
    pub fn with_keypair(privkey: Ed25519PrivateKey, pubkey: Ed25519PublicKey) -> Self {
        let addr = libra_types::account_address::from_public_key(&pubkey);
        Account {
            addr,
            privkey,
            pubkey,
        }
    }

    /// Creates a new account in memory representing an account created in the genesis transaction.
    ///
    /// The address will be [`address`], which should be an address for a genesis account and
    /// the account will use [`GENESIS_KEYPAIR`][struct@GENESIS_KEYPAIR] as its keypair.
    pub fn new_genesis_account(address: AccountAddress) -> Self {
        Account {
            addr: address,
            pubkey: GENESIS_KEYPAIR.1.clone(),
            privkey: GENESIS_KEYPAIR.0.clone(),
        }
    }

    /// Creates a new account representing the association in memory.
    ///
    /// The address will be [`association_address`][account_config::association_address], and
    /// the account will use [`GENESIS_KEYPAIR`][struct@GENESIS_KEYPAIR] as its keypair.
    pub fn new_association() -> Self {
        Self::new_genesis_account(account_config::association_address())
    }

    /// Creates a new account representing treasury compliance in memory.
    /// The account will use [`GENESIS_KEYPAIR`][struct@GENESIS_KEYPAIR] as its keypair.
    pub fn new_blessed_tc() -> Self {
        Self::new_genesis_account(account_config::treasury_compliance_account_address())
    }

    /// Returns the address of the account. This is a hash of the public key the account was created
    /// with.
    ///
    /// The address does not change if the account's [keys are rotated][Account::rotate_key].
    pub fn address(&self) -> &AccountAddress {
        &self.addr
    }

    /// Returns the AccessPath that describes the Account resource instance.
    ///
    /// Use this to retrieve or publish the Account blob.
    pub fn make_account_access_path(&self) -> AccessPath {
        self.make_access_path(AccountResource::struct_tag())
    }

    /// Returns the AccessPath that describes the EventHandleGenerator resource instance.
    ///
    /// Use this to retrieve or publish the EventHandleGenerator blob.
    pub fn make_event_generator_access_path(&self) -> AccessPath {
        self.make_access_path(account_config::event_handle_generator_struct_tag())
    }

    /// Returns the AccessPath that describes the Account balance resource instance.
    ///
    /// Use this to retrieve or publish the Account balance blob.
    pub fn make_balance_access_path(&self, balance_currency_code: Identifier) -> AccessPath {
        let type_tag = type_tag_for_currency_code(balance_currency_code);
        // TODO/XXX: Convert this to BalanceResource::struct_tag once that takes type args
        self.make_access_path(BalanceResource::struct_tag_for_currency(type_tag))
    }

    // TODO: plug in the account type
    pub fn make_access_path(&self, tag: StructTag) -> AccessPath {
        // TODO: we need a way to get the type (FatStructType) of the Account in place
        let resource_tag = ResourceKey::new(self.addr, tag);
        AccessPath::resource_access_path(&resource_tag)
    }

    /// Changes the keys for this account to the provided ones.
    pub fn rotate_key(&mut self, privkey: Ed25519PrivateKey, pubkey: Ed25519PublicKey) {
        self.privkey = privkey;
        self.pubkey = pubkey;
    }

    /// Computes the authentication key for this account, as stored on the chain.
    ///
    /// This is the same as the account's address if the keys have never been rotated.
    pub fn auth_key(&self) -> Vec<u8> {
        AuthenticationKey::ed25519(&self.pubkey).to_vec()
    }

    /// Return the first 16 bytes of the account's auth key
    pub fn auth_key_prefix(&self) -> Vec<u8> {
        AuthenticationKey::ed25519(&self.pubkey).prefix().to_vec()
    }

    //
    // Helpers to read data from an Account resource
    //

    //
    // Helpers for transaction creation with Account instance as sender
    //

    /// Returns a [`SignedTransaction`] with a payload and this account as the sender.
    ///
    /// This is the most generic way to create a transaction for testing.
    /// Max gas amount and gas unit price are ignored for WriteSet transactions.
    pub fn create_user_txn(
        &self,
        payload: TransactionPayload,
        sequence_number: u64,
        max_gas_amount: u64,
        gas_unit_price: u64,
        gas_currency_code: String,
    ) -> SignedTransaction {
        Self::create_raw_user_txn(
            *self.address(),
            payload,
            sequence_number,
            max_gas_amount,
            gas_unit_price,
            gas_currency_code,
        )
        .sign(&self.privkey, self.pubkey.clone())
        .unwrap()
        .into_inner()
    }

    pub fn create_raw_user_txn(
        address: AccountAddress,
        payload: TransactionPayload,
        sequence_number: u64,
        max_gas_amount: u64,
        gas_unit_price: u64,
        gas_currency_code: String,
    ) -> RawTransaction {
        match payload {
            TransactionPayload::WriteSet(writeset) => {
                RawTransaction::new_change_set(address, sequence_number, writeset)
            }
            TransactionPayload::Module(module) => RawTransaction::new_module(
                address,
                sequence_number,
                module,
                max_gas_amount,
                gas_unit_price,
                gas_currency_code,
                Duration::from_secs(DEFAULT_EXPIRATION_TIME),
            ),
            TransactionPayload::Script(script) => RawTransaction::new_script(
                address,
                sequence_number,
                script,
                max_gas_amount,
                gas_unit_price,
                gas_currency_code,
                Duration::from_secs(DEFAULT_EXPIRATION_TIME),
            ),
        }
    }

    /// Returns a [`SignedTransaction`] with the arguments defined in `args` and this account as
    /// the sender.
    pub fn create_signed_txn_with_args(
        &self,
        program: Vec<u8>,
        ty_args: Vec<TypeTag>,
        args: Vec<TransactionArgument>,
        sequence_number: u64,
        max_gas_amount: u64,
        gas_unit_price: u64,
        gas_currency_code: String,
    ) -> SignedTransaction {
        self.create_signed_txn_impl(
            *self.address(),
            TransactionPayload::Script(Script::new(program, ty_args, args)),
            sequence_number,
            max_gas_amount,
            gas_unit_price,
            gas_currency_code,
        )
    }

    pub fn create_raw_txn_with_args(
        address: AccountAddress,
        program: Vec<u8>,
        ty_args: Vec<TypeTag>,
        args: Vec<TransactionArgument>,
        sequence_number: u64,
        max_gas_amount: u64,
        gas_unit_price: u64,
        gas_currency_code: String,
    ) -> RawTransaction {
        Self::create_raw_txn_impl(
            address,
            TransactionPayload::Script(Script::new(program, ty_args, args)),
            sequence_number,
            max_gas_amount,
            gas_unit_price,
            gas_currency_code,
        )
    }

    /// Returns a [`SignedTransaction`] with the arguments defined in `args` and a custom sender.
    ///
    /// The transaction is signed with the key corresponding to this account, not the custom sender.
    pub fn create_signed_txn_with_args_and_sender(
        &self,
        sender: AccountAddress,
        program: Vec<u8>,
        ty_args: Vec<TypeTag>,
        args: Vec<TransactionArgument>,
        sequence_number: u64,
        max_gas_amount: u64,
        gas_unit_price: u64,
        gas_currency_code: String,
    ) -> SignedTransaction {
        self.create_signed_txn_impl(
            sender,
            TransactionPayload::Script(Script::new(program, ty_args, args)),
            sequence_number,
            max_gas_amount,
            gas_unit_price,
            gas_currency_code,
        )
    }

    pub fn create_raw_txn_with_args_and_sender(
        sender: AccountAddress,
        program: Vec<u8>,
        ty_args: Vec<TypeTag>,
        args: Vec<TransactionArgument>,
        sequence_number: u64,
        max_gas_amount: u64,
        gas_unit_price: u64,
        gas_currency_code: String,
    ) -> RawTransaction {
        Self::create_raw_txn_impl(
            sender,
            TransactionPayload::Script(Script::new(program, ty_args, args)),
            sequence_number,
            max_gas_amount,
            gas_unit_price,
            gas_currency_code,
        )
    }

    /// Returns a [`SignedTransaction`] with the arguments defined in `args` and a custom sender.
    ///
    /// The transaction is signed with the key corresponding to this account, not the custom sender.
    pub fn create_signed_txn_impl(
        &self,
        sender: AccountAddress,
        program: TransactionPayload,
        sequence_number: u64,
        max_gas_amount: u64,
        gas_unit_price: u64,
        gas_currency_code: String,
    ) -> SignedTransaction {
        Self::create_raw_txn_impl(
            sender,
            program,
            sequence_number,
            max_gas_amount,
            gas_unit_price,
            gas_currency_code,
        )
        .sign(&self.privkey, self.pubkey.clone())
        .unwrap()
        .into_inner()
    }

    /// Create a transaction containing `script` signed by `sender` with default values for gas
    /// cost, gas price, expiration time, and currency type.
    pub fn signed_script_txn(&self, script: Script, sequence_number: u64) -> SignedTransaction {
        self.create_signed_txn_impl(
            *self.address(),
            TransactionPayload::Script(script),
            sequence_number,
            gas_costs::TXN_RESERVED * 2,
            0, // gas price
            LBR_NAME.to_owned(),
        )
    }

    pub fn create_raw_txn_impl(
        sender: AccountAddress,
        program: TransactionPayload,
        sequence_number: u64,
        max_gas_amount: u64,
        gas_unit_price: u64,
        gas_currency_code: String,
    ) -> RawTransaction {
        RawTransaction::new(
            sender,
            sequence_number,
            program,
            max_gas_amount,
            gas_unit_price,
            gas_currency_code,
            // TTL is 86400s. Initial time was set to 0.
            Duration::from_secs(DEFAULT_EXPIRATION_TIME),
        )
    }
}

impl Default for Account {
    fn default() -> Self {
        Self::new()
    }
}

//---------------------------------------------------------------------------
// Balance resource represenation
//---------------------------------------------------------------------------

/// Struct that represents an account balance resource for tests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Balance {
    coin: u64,
}

impl Balance {
    /// Create a new balance with amount `balance`
    pub fn new(coin: u64) -> Self {
        Self { coin }
    }

    /// Retrieve the balance inside of this
    pub fn coin(&self) -> u64 {
        self.coin
    }

    /// Returns the Move Value for the account balance
    pub fn to_value(&self) -> Value {
        Value::struct_(Struct::pack(vec![Value::u64(self.coin)], true))
    }

    /// Returns the value layout for the account balance
    pub fn type_() -> FatStructType {
        FatStructType {
            address: account_config::CORE_CODE_ADDRESS,
            module: AccountResource::module_identifier(),
            name: BalanceResource::struct_identifier(),
            is_resource: true,
            ty_args: vec![],
            layout: vec![FatType::U64],
        }
    }
}

//---------------------------------------------------------------------------
// Account type represenation
//---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountRoleSpecifier {
    AssocRoot,
    TreasuryCompliance,
    DesignatedDealer,
    Validator,
    ValidatorOperator,
    ParentVASP,
    ChildVASP,
    Unhosted,
}

impl AccountRoleSpecifier {
    pub fn id(&self) -> u64 {
        match self {
            Self::AssocRoot => 0,
            Self::TreasuryCompliance => 1,
            Self::DesignatedDealer => 2,
            Self::Validator => 3,
            Self::ValidatorOperator => 4,
            Self::ParentVASP => 5,
            Self::ChildVASP => 6,
            Self::Unhosted => 7,
        }
    }
}

impl FromStr for AccountRoleSpecifier {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "empty" => Ok(AccountRoleSpecifier::AssocRoot), // TODO: rename from empty
            "unhosted" => Ok(AccountRoleSpecifier::Unhosted),
            "vasp" => Ok(AccountRoleSpecifier::ParentVASP), // TODO: rename from vasp
            other => Err(Error::msg(format!(
                "Unrecognized account type specifier {} found.",
                other
            ))),
        }
    }
}

impl Default for AccountRoleSpecifier {
    fn default() -> Self {
        AccountRoleSpecifier::ParentVASP
    }
}

//---------------------------------------------------------------------------
// Account type resource represenation
//---------------------------------------------------------------------------

/// Struct that represents an account type for testing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountRole {
    self_address: AccountAddress,
    account_specifier: AccountRoleSpecifier,
}

impl AccountRole {
    /// Create a new AccountRole testing account.
    pub fn new(self_address: AccountAddress, account_specifier: AccountRoleSpecifier) -> Self {
        Self {
            self_address,
            account_specifier,
        }
    }

    pub fn account_specifier(&self) -> AccountRoleSpecifier {
        self.account_specifier
    }
}

//---------------------------------------------------------------------------
// Event generator resource represenation
//---------------------------------------------------------------------------

/// Struct that represents the event generator resource stored under accounts

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventHandleGenerator {
    counter: u64,
    addr: AccountAddress,
}

impl EventHandleGenerator {
    pub fn new(addr: AccountAddress) -> Self {
        Self { addr, counter: 0 }
    }

    pub fn new_with_event_count(addr: AccountAddress, counter: u64) -> Self {
        Self { addr, counter }
    }

    pub fn to_value(&self) -> Value {
        Value::struct_(Struct::pack(
            vec![Value::u64(self.counter), Value::address(self.addr)],
            true,
        ))
    }

    pub fn type_() -> FatStructType {
        FatStructType {
            address: account_config::CORE_CODE_ADDRESS,
            module: account_config::event_module_name().to_owned(),
            name: account_config::event_handle_generator_struct_name().to_owned(),
            is_resource: true,
            ty_args: vec![],
            layout: vec![FatType::U64, FatType::Address],
        }
    }
}

/// Represents an account along with initial state about it.
///
/// `AccountData` captures the initial state needed to create accounts for tests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountData {
    account: Account,
    withdrawal_capability: Option<WithdrawCapability>,
    key_rotation_capability: Option<KeyRotationCapability>,
    sequence_number: u64,
    sent_events: EventHandle,
    received_events: EventHandle,
    is_frozen: bool,
    role_id: u64,

    balances: BTreeMap<Identifier, Balance>,
    event_generator: EventHandleGenerator,
    account_role: AccountRole,
}

fn new_event_handle(count: u64) -> EventHandle {
    EventHandle::random_handle(count)
}

impl AccountData {
    /// Creates a new `AccountData` with a new account.
    ///
    /// Most tests will want to use this constructor.
    pub fn new(balance: u64, sequence_number: u64) -> Self {
        Self::with_account(
            Account::new(),
            balance,
            lbr_currency_code(),
            sequence_number,
            AccountRoleSpecifier::ParentVASP,
        )
    }

    pub fn new_assoc_root() -> Self {
        Self::with_account(
            Account::new(),
            0,
            lbr_currency_code(),
            0,
            AccountRoleSpecifier::AssocRoot,
        )
    }

    pub fn new_unhosted() -> Self {
        Self::with_account(
            Account::new(),
            0,
            lbr_currency_code(),
            0,
            AccountRoleSpecifier::Unhosted,
        )
    }

    /// Creates a new `AccountData` with the provided account.
    pub fn with_account(
        account: Account,
        balance: u64,
        balance_currency_code: Identifier,
        sequence_number: u64,
        account_specifier: AccountRoleSpecifier,
    ) -> Self {
        Self::with_account_and_event_counts(
            account,
            balance,
            balance_currency_code,
            sequence_number,
            0,
            0,
            account_specifier,
            false,
        )
    }

    /// Creates a new `AccountData` with the provided account.
    pub fn with_keypair(
        privkey: Ed25519PrivateKey,
        pubkey: Ed25519PublicKey,
        balance: u64,
        balance_currency_code: Identifier,
        sequence_number: u64,
        account_specifier: AccountRoleSpecifier,
    ) -> Self {
        let account = Account::with_keypair(privkey, pubkey);
        Self::with_account(
            account,
            balance,
            balance_currency_code,
            sequence_number,
            account_specifier,
        )
    }

    /// Creates a new `AccountData` with custom parameters.
    pub fn with_account_and_event_counts(
        account: Account,
        balance: u64,
        balance_currency_code: Identifier,
        sequence_number: u64,
        sent_events_count: u64,
        received_events_count: u64,
        account_specifier: AccountRoleSpecifier,
        is_frozen: bool,
    ) -> Self {
        let mut balances = BTreeMap::new();
        balances.insert(balance_currency_code, Balance::new(balance));
        Self {
            account_role: AccountRole::new(*account.address(), account_specifier),
            event_generator: EventHandleGenerator::new_with_event_count(*account.address(), 2),
            withdrawal_capability: Some(WithdrawCapability::new(*account.address())),
            key_rotation_capability: Some(KeyRotationCapability::new(*account.address())),
            account,
            balances,
            sequence_number,
            is_frozen,
            sent_events: new_event_handle(sent_events_count),
            received_events: new_event_handle(received_events_count),
            role_id: account_specifier.id(),
        }
    }

    /// Adds the balance held by this account to the one represented as balance_currency_code
    pub fn add_balance_currency(&mut self, balance_currency_code: Identifier) {
        self.balances.insert(balance_currency_code, Balance::new(0));
    }

    /// Changes the keys for this account to the provided ones.
    pub fn rotate_key(&mut self, privkey: Ed25519PrivateKey, pubkey: Ed25519PublicKey) {
        self.account.rotate_key(privkey, pubkey)
    }

    pub fn sent_payment_event_type() -> FatStructType {
        FatStructType {
            address: account_config::CORE_CODE_ADDRESS,
            module: AccountResource::module_identifier(),
            name: SentPaymentEvent::struct_identifier(),
            is_resource: false,
            ty_args: vec![],
            layout: vec![
                FatType::U64,
                FatType::Address,
                FatType::Vector(Box::new(FatType::U8)),
            ],
        }
    }

    pub fn received_payment_event_type() -> FatStructType {
        FatStructType {
            address: account_config::CORE_CODE_ADDRESS,
            module: AccountResource::module_identifier(),
            name: ReceivedPaymentEvent::struct_identifier(),
            is_resource: false,
            ty_args: vec![],
            layout: vec![
                FatType::U64,
                FatType::Address,
                FatType::Vector(Box::new(FatType::U8)),
            ],
        }
    }

    pub fn event_handle_type(ty: FatType) -> FatStructType {
        FatStructType {
            address: account_config::CORE_CODE_ADDRESS,
            module: account_config::event_module_name().to_owned(),
            name: account_config::event_handle_struct_name().to_owned(),
            is_resource: true,
            ty_args: vec![ty],
            layout: vec![FatType::U64, FatType::Vector(Box::new(FatType::U8))],
        }
    }

    /// Returns the (Move value) layout of the LibraAccount::LibraAccount struct
    pub fn type_() -> FatStructType {
        FatStructType {
            address: account_config::CORE_CODE_ADDRESS,
            module: AccountResource::module_identifier(),
            name: AccountResource::struct_identifier(),
            is_resource: true,
            ty_args: vec![],
            layout: vec![
                FatType::Vector(Box::new(FatType::U8)),
                FatType::Vector(Box::new(FatType::Struct(Box::new(
                    WithdrawCapability::type_(),
                )))),
                FatType::Vector(Box::new(FatType::Struct(Box::new(
                    KeyRotationCapability::type_(),
                )))),
                FatType::Struct(Box::new(Self::event_handle_type(FatType::Struct(
                    Box::new(Self::sent_payment_event_type()),
                )))),
                FatType::Struct(Box::new(Self::event_handle_type(FatType::Struct(
                    Box::new(Self::received_payment_event_type()),
                )))),
                FatType::U64,
                FatType::Bool,
                FatType::U64,
            ],
        }
    }

    /// Returns whether the underlying account is an an empty account type or not.
    pub fn account_role(&self) -> AccountRoleSpecifier {
        self.account_role.account_specifier()
    }

    /// Creates and returns the top-level resources to be published under the account
    pub fn to_value(&self) -> (Value, Vec<(Identifier, Value)>, Value) {
        // TODO: publish some concept of Account
        let balances: Vec<_> = self
            .balances
            .iter()
            .map(|(code, balance)| (code.clone(), balance.to_value()))
            .collect();
        let event_generator = self.event_generator.to_value();
        let account = Value::struct_(Struct::pack(
            vec![
                // TODO: this needs to compute the auth key instead
                Value::vector_u8(AuthenticationKey::ed25519(&self.account.pubkey).to_vec()),
                self.withdrawal_capability.as_ref().unwrap().value(),
                self.key_rotation_capability.as_ref().unwrap().value(),
                Value::struct_(Struct::pack(
                    vec![
                        Value::u64(self.received_events.count()),
                        Value::vector_u8(self.received_events.key().to_vec()),
                    ],
                    true,
                )),
                Value::struct_(Struct::pack(
                    vec![
                        Value::u64(self.sent_events.count()),
                        Value::vector_u8(self.sent_events.key().to_vec()),
                    ],
                    true,
                )),
                Value::u64(self.sequence_number),
                Value::bool(self.is_frozen),
                Value::u64(self.role_id),
            ],
            true,
        ));
        (account, balances, event_generator)
    }

    /// Returns the AccessPath that describes the Account resource instance.
    ///
    /// Use this to retrieve or publish the Account blob.
    pub fn make_account_access_path(&self) -> AccessPath {
        self.account.make_account_access_path()
    }

    /// Returns the AccessPath that describes the Account balance resource instance.
    ///
    /// Use this to retrieve or publish the Account blob.
    pub fn make_balance_access_path(&self, code: Identifier) -> AccessPath {
        self.account.make_balance_access_path(code)
    }

    /// Returns the AccessPath that describes the EventHandleGenerator resource instance.
    ///
    /// Use this to retrieve or publish the EventHandleGenerator blob.
    pub fn make_event_generator_access_path(&self) -> AccessPath {
        self.account.make_event_generator_access_path()
    }

    /// Creates a writeset that contains the account data and can be patched to the storage
    /// directly.
    pub fn to_writeset(&self) -> WriteSet {
        let (account_blob, balance_blobs, event_generator_blob) = self.to_value();
        let mut write_set = Vec::new();
        let account = account_blob
            .value_as::<Struct>()
            .unwrap()
            .simple_serialize(&AccountData::type_())
            .unwrap();
        write_set.push((self.make_account_access_path(), WriteOp::Value(account)));
        for (code, balance_blob) in balance_blobs.into_iter() {
            let balance = balance_blob
                .value_as::<Struct>()
                .unwrap()
                .simple_serialize(&Balance::type_())
                .unwrap();
            write_set.push((self.make_balance_access_path(code), WriteOp::Value(balance)));
        }

        let event_generator = event_generator_blob
            .value_as::<Struct>()
            .unwrap()
            .simple_serialize(&EventHandleGenerator::type_())
            .unwrap();
        write_set.push((
            self.make_event_generator_access_path(),
            WriteOp::Value(event_generator),
        ));
        WriteSetMut::new(write_set).freeze().unwrap()
    }

    /// Returns the address of the account. This is a hash of the public key the account was created
    /// with.
    ///
    /// The address does not change if the account's [keys are rotated][AccountData::rotate_key].
    pub fn address(&self) -> &AccountAddress {
        self.account.address()
    }

    /// Returns the underlying [`Account`] instance.
    pub fn account(&self) -> &Account {
        &self.account
    }

    /// Converts this data into an `Account` instance.
    pub fn into_account(self) -> Account {
        self.account
    }

    /// Returns the initial balance.
    pub fn balance(&self, currency_code: &IdentStr) -> u64 {
        self.balances.get(currency_code).unwrap().coin()
    }

    /// Returns the initial sequence number.
    pub fn sequence_number(&self) -> u64 {
        self.sequence_number
    }

    /// Returns the unique key for this sent events stream.
    pub fn sent_events_key(&self) -> &[u8] {
        self.sent_events.key().as_bytes()
    }

    /// Returns the initial sent events count.
    pub fn sent_events_count(&self) -> u64 {
        self.sent_events.count()
    }

    /// Returns the unique key for this received events stream.
    pub fn received_events_key(&self) -> &[u8] {
        self.received_events.key().as_bytes()
    }

    /// Returns the initial received events count.
    pub fn received_events_count(&self) -> u64 {
        self.received_events.count()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WithdrawCapability {
    account_address: AccountAddress,
}
impl WithdrawCapability {
    pub fn new(account_address: AccountAddress) -> Self {
        Self { account_address }
    }

    pub fn type_() -> FatStructType {
        FatStructType {
            address: account_config::CORE_CODE_ADDRESS,
            module: WithdrawCapabilityResource::module_identifier(),
            name: WithdrawCapabilityResource::struct_identifier(),
            is_resource: true,
            ty_args: vec![],
            layout: vec![FatType::Address],
        }
    }

    pub fn value(&self) -> Value {
        Value::vector_general(vec![Value::struct_(Struct::pack(
            vec![Value::address(self.account_address)],
            true,
        ))])
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyRotationCapability {
    account_address: AccountAddress,
}
impl KeyRotationCapability {
    pub fn new(account_address: AccountAddress) -> Self {
        Self { account_address }
    }

    pub fn type_() -> FatStructType {
        FatStructType {
            address: account_config::CORE_CODE_ADDRESS,
            module: KeyRotationCapabilityResource::module_identifier(),
            name: KeyRotationCapabilityResource::struct_identifier(),
            is_resource: true,
            ty_args: vec![],
            layout: vec![FatType::Address],
        }
    }

    pub fn value(&self) -> Value {
        Value::vector_general(vec![Value::struct_(Struct::pack(
            vec![Value::address(self.account_address)],
            true,
        ))])
    }
}
