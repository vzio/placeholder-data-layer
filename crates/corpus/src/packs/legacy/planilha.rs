//! A customer spreadsheet imported column for column as text: pt-BR
//! decimals, numbers that lost their leading zeros or collapsed into
//! scientific notation, text forced with a leading apostrophe, dates as
//! serial numbers, spreadsheet error values, the header row repeated where
//! sheets were pasted together, and blank rows at the end.

use anonymizer::Value;
use serde::Deserialize;
use server::http::dto::{DatasetDto, WireType};

use super::legacy_text::{decimal_br, scientific_br, spreadsheet_serial};
use crate::generate::docs::{Document, text_of, without_leading_zeros};
use crate::generate::person::{Person, cep_masked, phone_masked};
use crate::generate::rng::Rng;
use crate::generate::table::{TableBuilder, text};
use crate::generate::{GenError, params};

/// The sheet's header, which is also the table's column names.
const HEADER: &[&str] = &[
    "Nome",
    "CPF",
    "Celular",
    "E-mail",
    "Data Nasc",
    "Valor Compras",
    "Qtd Pedidos",
    "Cidade",
    "UF",
    "CEP",
    "Observação",
];

const NOTES: &[&str] = &[
    "cliente vip",
    "ligar depois das 14h",
    "prefere whatsapp",
    "comprou na feira",
    "indicação da Márcia",
    "#N/D",
    "#REF!",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanilhaParams {
    linhas: usize,
    cabecalhos_repetidos: usize,
    linhas_vazias: usize,
}

fn sheet_cpf(rng: &mut Rng, cpf: &[u8]) -> String {
    match rng.below(100) {
        0..=44 => Document::Cpf.masked(cpf),
        45..=59 => format!("'{}", Document::Cpf.masked(cpf)),
        60..=79 => without_leading_zeros(cpf),
        80..=86 => scientific_br(&text_of(cpf)),
        87..=94 => String::new(),
        _ => "-".to_owned(),
    }
}

fn sheet_phone(rng: &mut Rng, digits: &str) -> String {
    match rng.below(100) {
        0..=34 => phone_masked(digits),
        35..=54 => format!("'{digits}"),
        55..=79 => scientific_br(digits),
        80..=89 => digits.to_owned(),
        _ => String::new(),
    }
}

fn sheet_email(rng: &mut Rng, email: &str) -> String {
    match rng.below(100) {
        0..=79 => email.to_owned(),
        80..=86 => format!("{email} "),
        87..=91 => email.to_uppercase(),
        _ => String::new(),
    }
}

fn sheet_birth(rng: &mut Rng, (year, month, day): (u32, u32, u32)) -> String {
    match rng.below(100) {
        0..=44 => format!("{day:02}/{month:02}/{year:04}"),
        45..=74 => spreadsheet_serial((year, month, day)).to_string(),
        75..=89 => format!("{day:02}/{month:02}/{:02}", year % 100),
        _ => String::new(),
    }
}

fn sheet_money(rng: &mut Rng) -> String {
    let cents = rng.between(1_990, 2_500_000);
    match rng.below(100) {
        0..=49 => decimal_br(cents),
        50..=74 => format!("R$ {}", decimal_br(cents)),
        75..=89 => format!("{},{:02}", cents / 100, cents % 100),
        90..=94 => "-".to_owned(),
        _ => String::new(),
    }
}

fn sheet_cep(rng: &mut Rng, cep: &str) -> String {
    match rng.below(100) {
        0..=54 => cep_masked(cep),
        55..=79 => cep.trim_start_matches('0').to_owned(),
        80..=92 => format!("'{cep}"),
        _ => String::new(),
    }
}

fn customer_row(rng: &mut Rng) -> Vec<Value> {
    let person = Person::generate(rng);
    let orders = rng.between(1, 40);
    let quantity = if rng.chance(300) {
        format!("{orders},0")
    } else {
        orders.to_string()
    };
    let note = if rng.chance(250) {
        rng.pick(NOTES).copied().unwrap_or("#N/D").to_owned()
    } else {
        String::new()
    };
    let name = if rng.chance(120) {
        format!("{} ", person.full_name)
    } else {
        person.full_name.clone()
    };
    vec![
        text(name),
        text(sheet_cpf(rng, &person.cpf)),
        text(sheet_phone(rng, &person.phone)),
        text(sheet_email(rng, &person.email)),
        text(sheet_birth(rng, person.birth)),
        text(sheet_money(rng)),
        text(quantity),
        text(person.city.name),
        text(person.city.uf),
        text(sheet_cep(rng, &person.cep)),
        text(note),
    ]
}

pub fn planilha_importada_excel(
    seed: u64,
    raw: &serde_json::Value,
) -> Result<DatasetDto, GenError> {
    let p: PlanilhaParams = params(raw)?;
    if p.cabecalhos_repetidos >= p.linhas {
        return Err(GenError("more repeated headers than rows".to_owned()));
    }
    let root = Rng::new(seed);
    let mut rng = root.fork("planilha");
    let mut rows: Vec<Vec<Value>> = (0..p.linhas).map(|_| customer_row(&mut rng)).collect();

    let mut header_rng = root.fork("cabecalhos");
    let sheet_length = p.linhas / (p.cabecalhos_repetidos + 1);
    for sheet in (1..=p.cabecalhos_repetidos).rev() {
        let at = sheet * sheet_length + header_rng.index(5);
        let header = HEADER.iter().map(|h| text(*h)).collect();
        rows.insert(at.min(rows.len()), header);
    }
    for _ in 0..p.linhas_vazias {
        rows.push(HEADER.iter().map(|_| text("")).collect());
    }

    let mut table = TableBuilder::new("clientes_planilha");
    for name in HEADER {
        table = table.column(name, WireType::String);
    }
    for row in rows {
        table.push(row)?;
    }
    Ok(DatasetDto {
        tables: vec![table.build()],
    })
}
