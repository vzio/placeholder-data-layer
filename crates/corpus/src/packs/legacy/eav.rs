//! Generic columns whose meaning lives outside the schema: an
//! entity-attribute-value table whose one value column holds emails, CPFs,
//! CNPJs, dates, phones, product data and free text side by side, and a
//! registration table of twenty columns named `campo1` to `campo20`.

use anonymizer::Value;
use serde::Deserialize;
use server::http::dto::{DatasetDto, WireType};

use crate::generate::company::Company;
use crate::generate::docs::{Document, text_of};
use crate::generate::mess::{PLAIN_NOTES, date_br, date_iso, note_embedding};
use crate::generate::person::{Person, cep_masked, phone_masked};
use crate::generate::rng::Rng;
use crate::generate::table::{TableBuilder, maybe_text, text};
use crate::generate::{GenError, params};

const ENTITY_KINDS: &[(&str, u64)] = &[("cliente", 60), ("fornecedor", 15), ("produto", 25)];

const CHANNELS: &[&str] = &["site", "loja", "telefone", "app"];

const COLOURS: &[&str] = &["preto", "branco", "azul", "vermelho", "cinza", "verde"];

const PRODUCTS: &[&str] = &[
    "Cadeira de escritório",
    "Mesa dobrável",
    "Luminária de mesa",
    "Estante de aço",
    "Arquivo de 4 gavetas",
    "Quadro branco 90x60",
];

const STATUS_CODES: &[(&str, u64)] = &[("A", 80), ("I", 15), ("B", 5)];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EavParams {
    entidades: usize,
    cadastros: usize,
}

/// Each attribute an entity of this kind may carry, with its value. An
/// attribute is left out one time in eight.
fn attributes(rng: &mut Rng, kind: &str) -> Vec<(&'static str, String)> {
    let mut values = Vec::new();
    match kind {
        "cliente" => {
            let person = Person::generate(rng);
            let cpf = if rng.chance(700) {
                Document::Cpf.masked(&person.cpf)
            } else {
                text_of(&person.cpf)
            };
            let birth = if rng.chance(600) {
                date_iso(person.birth)
            } else {
                date_br(person.birth)
            };
            let note = if rng.chance(400) {
                note_embedding(
                    rng,
                    &Document::Cpf.masked(&person.cpf),
                    &phone_masked(&person.phone),
                )
            } else {
                rng.pick(PLAIN_NOTES)
                    .copied()
                    .unwrap_or_default()
                    .to_owned()
            };
            values.push(("nome", person.full_name.clone()));
            values.push(("email", person.email.clone()));
            values.push(("cpf", cpf));
            values.push(("data_nascimento", birth));
            values.push(("telefone", phone_masked(&person.phone)));
            values.push((
                "canal",
                rng.pick(CHANNELS).copied().unwrap_or("site").to_owned(),
            ));
            values.push(("obs", note));
        }
        "fornecedor" => {
            let company = Company::generate(rng);
            values.push(("razao_social", company.razao_social.clone()));
            values.push(("cnpj", Document::Cnpj.masked(&company.cnpj)));
            values.push(("email", company.email.clone()));
            values.push(("telefone", phone_masked(&company.phone)));
            values.push(("cep", cep_masked(&company.cep)));
        }
        _ => {
            let product = rng.pick(PRODUCTS).copied().unwrap_or("Mesa dobrável");
            values.push(("descricao", product.to_owned()));
            values.push((
                "cor",
                rng.pick(COLOURS).copied().unwrap_or("preto").to_owned(),
            ));
            let grams = rng.between(300, 25_000);
            values.push(("peso_kg", format!("{},{:03}", grams / 1000, grams % 1000)));
            values.push(("sku", format!("MOV-{:05}", rng.between(1, 99_999))));
        }
    }
    values.retain(|_| !rng.chance(125));
    values
}

fn generic_row(rng: &mut Rng, id: i64, reused_half: bool) -> Vec<Value> {
    let person = Person::generate(rng);
    let note = if rng.chance(300) {
        Some(note_embedding(
            rng,
            &Document::Cpf.masked(&person.cpf),
            &phone_masked(&person.phone),
        ))
    } else if rng.chance(500) {
        rng.pick(PLAIN_NOTES).map(|n| (*n).to_owned())
    } else {
        None
    };
    // `campo13` held phones until the form was redesigned, then emails.
    let campo13 = if reused_half {
        person.email.clone()
    } else {
        phone_masked(&person.phone)
    };
    let updated = (
        rng.between(2019, 2024) as u32,
        rng.between(1, 12) as u32,
        rng.between(1, 28) as u32,
    );
    let cents = rng.between(1_000, 900_000);
    vec![
        Value::Int(id),
        text(person.full_name.clone()),
        text(Document::Cpf.masked(&person.cpf)),
        text(person.email.clone()),
        text(phone_masked(&person.phone)),
        text(date_br(person.birth)),
        text(cep_masked(&person.cep)),
        text(person.city.name),
        text(person.city.uf),
        text(rng.weighted(STATUS_CODES).copied().unwrap_or("A")),
        text(format!("{}.{:02}", cents / 100, cents % 100)),
        maybe_text(note),
        text("0"),
        text(campo13),
        Value::Null,
        text(if rng.chance(30) { "X" } else { "" }),
        text(format!("CLI-{:06}", id)),
        text(date_iso(updated)),
        if rng.chance(50) {
            text("ver planilha antiga")
        } else {
            Value::Null
        },
        text(if rng.chance(900) { "S" } else { "N" }),
        text("3"),
    ]
}

pub fn colunas_genericas_eav(seed: u64, raw: &serde_json::Value) -> Result<DatasetDto, GenError> {
    let p: EavParams = params(raw)?;
    let root = Rng::new(seed);
    let mut rng = root.fork("entidade");
    let mut entidade = TableBuilder::new("entidade")
        .primary_key("id", WireType::Integer)
        .column("tipo", WireType::String)
        .column("criado_em", WireType::String);
    let mut atributo_valor = TableBuilder::new("atributo_valor")
        .primary_key("id", WireType::Integer)
        .foreign_key("entidade_id", WireType::Integer, "entidade", "id")
        .column("atributo", WireType::String)
        .column("valor", WireType::String);
    for index in 0..p.entidades {
        let id = index as i64 + 1;
        let kind = rng.weighted(ENTITY_KINDS).copied().unwrap_or("cliente");
        let created = (
            rng.between(2016, 2024) as u32,
            rng.between(1, 12) as u32,
            rng.between(1, 28) as u32,
        );
        entidade.push(vec![Value::Int(id), text(kind), text(date_iso(created))])?;
        for (attribute, value) in attributes(&mut rng, kind) {
            let row = atributo_valor.len() as i64 + 1;
            atributo_valor.push(vec![
                Value::Int(row),
                Value::Int(id),
                text(attribute),
                text(value),
            ])?;
        }
    }

    let mut rng = root.fork("cadastro_generico");
    let mut cadastro = TableBuilder::new("cadastro_generico").primary_key("id", WireType::Integer);
    for n in 1..=20 {
        cadastro = cadastro.column(&format!("campo{n}"), WireType::String);
    }
    for index in 0..p.cadastros {
        let reused_half = index >= p.cadastros / 2;
        cadastro.push(generic_row(&mut rng, index as i64 + 1, reused_half))?;
    }

    Ok(DatasetDto {
        tables: vec![entidade.build(), atributo_valor.build(), cadastro.build()],
    })
}
