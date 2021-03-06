// Copyright (c) The Libra Core Contributors
// SPDX-License-Identifier: Apache-2.0

use crate::type_not_allowed;
use libra_types::transaction::{ArgumentABI, ScriptABI, TypeArgumentABI};
use move_core_types::language_storage::TypeTag;

use std::io::{Result, Write};

pub fn output(out: &mut dyn Write, abis: &[ScriptABI]) -> Result<()> {
    output_preamble(out)?;
    for abi in abis {
        output_builder(out, abi)?;
    }
    Ok(())
}

fn output_preamble(out: &mut dyn Write) -> Result<()> {
    writeln!(out, "use libra_types as libra;",)
}

fn output_builder(out: &mut dyn Write, abi: &ScriptABI) -> Result<()> {
    write!(out, "\n{}", quote_doc(abi.doc()))?;
    writeln!(
        out,
        "pub fn encode_{}_script({}) -> libra::Script {{",
        abi.name(),
        [
            quote_type_parameters(abi.ty_args()),
            quote_parameters(abi.args()),
        ]
        .concat()
        .join(", ")
    )?;
    writeln!(
        out,
        r#"    libra::Script {{
        code: {},
        ty_args: vec![{}],
        args: vec![{}],
    }}"#,
        quote_code(abi.code()),
        quote_type_arguments(abi.ty_args()),
        quote_arguments(abi.args()),
    )?;
    writeln!(out, "}}")?;
    Ok(())
}

fn quote_doc(doc: &str) -> String {
    let text = textwrap::fill(doc, 86);
    textwrap::indent(&text, "/// ")
}

fn quote_type_parameters(ty_args: &[TypeArgumentABI]) -> Vec<String> {
    ty_args
        .iter()
        .map(|ty_arg| format!("{}: libra::TypeTag", ty_arg.name()))
        .collect()
}

fn quote_parameters(args: &[ArgumentABI]) -> Vec<String> {
    args.iter()
        .map(|arg| format!("{}: {}", arg.name(), quote_type(arg.type_tag())))
        .collect()
}

fn quote_code(code: &[u8]) -> String {
    format!(
        "vec![{}]",
        code.iter()
            .map(|x| format!("{}", x))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn quote_type_arguments(ty_args: &[TypeArgumentABI]) -> String {
    ty_args
        .iter()
        .map(|ty_arg| ty_arg.name().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn quote_arguments(args: &[ArgumentABI]) -> String {
    args.iter()
        .map(|arg| make_transaction_argument(arg.type_tag(), arg.name()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn quote_type(type_tag: &TypeTag) -> String {
    use TypeTag::*;
    match type_tag {
        Bool => "bool".into(),
        U8 => "u8".into(),
        U64 => "u64".into(),
        U128 => "u128".into(),
        Address => "libra::AccountAddress".into(),
        Vector(type_tag) => match type_tag.as_ref() {
            U8 => "Vec<u8>".into(),
            _ => type_not_allowed(type_tag),
        },

        Struct(_) | Signer => type_not_allowed(type_tag),
    }
}

fn make_transaction_argument(type_tag: &TypeTag, name: &str) -> String {
    use TypeTag::*;
    match type_tag {
        Bool => format!("libra::TransactionArgument::Bool({})", name),
        U8 => format!("libra::TransactionArgument::U8({})", name),
        U64 => format!("libra::TransactionArgument::U64({})", name),
        U128 => format!("libra::TransactionArgument::U128({})", name),
        Address => format!("libra::TransactionArgument::Address({})", name),
        Vector(type_tag) => match type_tag.as_ref() {
            U8 => format!("libra::TransactionArgument::U8Vector({})", name),
            _ => type_not_allowed(type_tag),
        },

        Struct(_) | Signer => type_not_allowed(type_tag),
    }
}
