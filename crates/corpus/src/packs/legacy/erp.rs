//! A customer master and its receivables from a terminal-era ERP: upper-case
//! fixed-width text, CPFs and CEPs stored as integers that lost their
//! leading zeros, dates as `YYYYMMDD` integers with `0` for unknown, text
//! sentinels in place of NULL, and Windows-1252 mojibake on every row
//! migrated from the system before it.

use anonymizer::Value;
use serde::Deserialize;
use server::http::dto::{DatasetDto, WireType};

use super::legacy_text::{fixed_width, mojibake_cp1252, random_date, upper_ascii, yyyymmdd};
use crate::generate::docs::{Document, text_of};
use crate::generate::person::Person;
use crate::generate::rng::Rng;
use crate::generate::table::{TableBuilder, maybe_text, text};
use crate::generate::{GenError, params};

const NAME_WIDTH: usize = 40;
const ADDRESS_WIDTH: usize = 50;
const PLACE_WIDTH: usize = 30;
const PHONE_WIDTH: usize = 15;

/// What the old system typed where it had nothing to say.
const TEXT_SENTINELS: &[&str] = &["NULL", "N/A", "-", "0"];

const STOCK_REMARKS: &[&str] = &[
    "CLIENTE ANTIGO",
    "NAO VENDER A PRAZO",
    "COBRAR NO ESCRITORIO",
    "ENDERECO CONFIRMADO",
    "VER COM GERENTE",
    "PAGA EM DIA",
];

const TITLE_STATUS: &[(&str, u64)] = &[("P", 70), ("A", 22), ("C", 8)];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ErpParams {
    clientes: usize,
    /// Share of customers, from the first, migrated from the previous system.
    migrados_por_mil: u64,
    titulos: usize,
    orfaos: usize,
}

/// Upper case as the migrated system stored it: accents kept and then
/// mangled by the migration, or folded to ASCII by the current one.
fn legacy_upper(text: &str, migrated: bool) -> String {
    if migrated {
        mojibake_cp1252(&text.to_uppercase())
    } else {
        upper_ascii(text)
    }
}

fn digits_as_integer(digits: &str) -> Value {
    digits.parse::<i64>().map_or(Value::Null, Value::Int)
}

fn legacy_cpf(rng: &mut Rng, cpf: &[u8]) -> Value {
    match rng.below(100) {
        0..=83 => digits_as_integer(&text_of(cpf)),
        84..=89 => Value::Int(0),
        90..=94 => Value::Null,
        _ => digits_as_integer(&text_of(&Document::Cpf.generate_invalid(rng))),
    }
}

fn legacy_birth(rng: &mut Rng, birth: (u32, u32, u32)) -> Value {
    match rng.below(100) {
        0..=84 => Value::Int(yyyymmdd(birth)),
        85..=90 => Value::Int(0),
        91..=93 => Value::Int(19_000_101),
        _ => Value::Null,
    }
}

fn legacy_cep(rng: &mut Rng, cep: &str) -> Value {
    match rng.below(100) {
        0..=93 => digits_as_integer(cep),
        94..=96 => Value::Int(0),
        _ => Value::Null,
    }
}

fn legacy_phone(rng: &mut Rng, digits: &str) -> Option<String> {
    let (ddd, number) = digits.split_at(2);
    let raw = match rng.below(100) {
        0..=44 => {
            let split = number.len() - 4;
            format!("(0{ddd}){}-{}", &number[..split], &number[split..])
        }
        45..=79 => digits.to_owned(),
        80..=91 => rng.pick(TEXT_SENTINELS).copied().unwrap_or("-").to_owned(),
        _ => return None,
    };
    Some(fixed_width(&raw, PHONE_WIDTH))
}

fn legacy_email(rng: &mut Rng, email: &str) -> Option<String> {
    match rng.below(100) {
        0..=44 => None,
        45..=79 => Some(email.to_uppercase()),
        80..=89 => Some(email.to_owned()),
        _ => rng.pick(&TEXT_SENTINELS[..3]).map(|s| (*s).to_owned()),
    }
}

fn legacy_remark(rng: &mut Rng, cpf: &[u8], phone: &str) -> Option<String> {
    match rng.below(100) {
        0..=69 => None,
        70..=89 => rng.pick(STOCK_REMARKS).map(|r| (*r).to_owned()),
        90..=94 => Some(format!("CPF {} CONF. NO BALCAO", text_of(cpf))),
        _ => Some(format!("FONE RECADO {phone}")),
    }
}

/// `date` moved forward by `months`, the day kept (never past the 28th).
fn months_later((year, month, day): (u32, u32, u32), months: u32) -> (u32, u32, u32) {
    let zero_based = month - 1 + months;
    (year + zero_based / 12, zero_based % 12 + 1, day)
}

fn customer_row(rng: &mut Rng, id: i64, migrated: bool) -> Vec<Value> {
    let person = Person::generate(rng);
    let registered = if migrated {
        random_date(rng, 1992, 2008)
    } else {
        random_date(rng, 2009, 2013)
    };
    let street = person.street.replace(',', "");
    vec![
        Value::Int(id),
        text(fixed_width(
            &legacy_upper(&person.full_name, migrated),
            NAME_WIDTH,
        )),
        legacy_cpf(rng, &person.cpf),
        legacy_birth(rng, person.birth),
        text(fixed_width(&legacy_upper(&street, migrated), ADDRESS_WIDTH)),
        text(fixed_width(
            &legacy_upper(person.bairro, migrated),
            PLACE_WIDTH,
        )),
        text(fixed_width(
            &legacy_upper(person.city.name, migrated),
            PLACE_WIDTH,
        )),
        text(person.city.uf),
        legacy_cep(rng, &person.cep),
        maybe_text(legacy_phone(rng, &person.phone)),
        maybe_text(legacy_email(rng, &person.email)),
        Value::Int(yyyymmdd(registered)),
        text(if rng.chance(820) { "S" } else { "N" }),
        maybe_text(legacy_remark(rng, &person.cpf, &person.phone)),
    ]
}

pub fn erp_legado_latin1(seed: u64, raw: &serde_json::Value) -> Result<DatasetDto, GenError> {
    let p: ErpParams = params(raw)?;
    if p.orfaos > p.titulos || p.clientes == 0 {
        return Err(GenError(
            "orphans need more receivables, and receivables need customers".to_owned(),
        ));
    }
    let root = Rng::new(seed);
    let mut rng = root.fork("cad_cliente");
    let migrated_until = p.clientes as u64 * p.migrados_por_mil / 1000;
    let mut clientes = TableBuilder::new("cad_cliente")
        .primary_key("cd_cliente", WireType::Integer)
        .column("nm_cliente", WireType::String)
        .column("nr_cpf", WireType::Integer)
        .column("dt_nascimento", WireType::Integer)
        .column("ds_endereco", WireType::String)
        .column("nm_bairro", WireType::String)
        .column("nm_cidade", WireType::String)
        .column("sg_uf", WireType::String)
        .column("nr_cep", WireType::Integer)
        .column("nr_telefone", WireType::String)
        .column("ds_email", WireType::String)
        .column("dt_cadastro", WireType::Integer)
        .column("fl_ativo", WireType::String)
        .column("ds_observacao", WireType::String);
    for index in 0..p.clientes {
        let migrated = (index as u64) < migrated_until;
        clientes.push(customer_row(&mut rng, index as i64 + 1, migrated))?;
    }

    let mut rng = root.fork("mov_titulo");
    let mut orphan_rows = Vec::new();
    while orphan_rows.len() < p.orfaos {
        let row = rng.index(p.titulos);
        if !orphan_rows.contains(&row) {
            orphan_rows.push(row);
        }
    }
    let mut titulos = TableBuilder::new("mov_titulo")
        .primary_key("nr_titulo", WireType::Integer)
        .foreign_key("cd_cliente", WireType::Integer, "cad_cliente", "cd_cliente")
        .column("dt_emissao", WireType::Integer)
        .column("dt_vencimento", WireType::Integer)
        .column("vl_titulo", WireType::Float)
        .column("st_titulo", WireType::String);
    for row in 0..p.titulos {
        let customer = if orphan_rows.contains(&row) {
            (p.clientes as u64 + rng.between(1, 500)) as i64
        } else {
            rng.between(1, p.clientes as u64) as i64
        };
        let issued = random_date(&mut rng, 2005, 2013);
        let due = months_later(issued, rng.between(1, 3) as u32);
        let cents = rng.between(1_500, 250_000);
        let status = rng.weighted(TITLE_STATUS).copied().unwrap_or("P");
        titulos.push(vec![
            Value::Int(500_000 + row as i64),
            Value::Int(customer),
            Value::Int(yyyymmdd(issued)),
            Value::Int(yyyymmdd(due)),
            Value::Float(cents as f64 / 100.0),
            text(status),
        ])?;
    }

    Ok(DatasetDto {
        tables: vec![clientes.build(), titulos.build()],
    })
}
