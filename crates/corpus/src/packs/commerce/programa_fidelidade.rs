//! A loyalty programme: members with their birth date and the last four
//! digits of a linked card, and a points ledger whose entries per member
//! follow a heavy tail — most members have a handful, a few have hundreds.

use anonymizer::Value;
use serde::Deserialize;
use server::http::dto::{DatasetDto, WireType};

use super::{cpf_in_either_layout, day, weighted};
use crate::generate::mess::{IMPOSSIBLE_DATES, date_br, date_iso};
use crate::generate::person::{Person, phone_masked};
use crate::generate::rng::Rng;
use crate::generate::table::{TableBuilder, maybe_text, text};
use crate::generate::{GenError, params};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FidelidadeParams {
    membros: usize,
    /// The heavy tail's scale: a member's entry count is about this over a
    /// uniform rank in `1..=1000`, capped by nothing else.
    cauda_lancamentos: u64,
}

const TIERS: &[(&str, u64)] = &[
    ("bronze", 620),
    ("prata", 240),
    ("ouro", 110),
    ("diamante", 30),
];

const ENTRY_KINDS: &[(&str, u64)] = &[
    ("acumulo", 780),
    ("resgate", 140),
    ("expiracao", 50),
    ("ajuste", 30),
];

const VOUCHERS: &[(&str, i64)] = &[
    ("Resgate voucher R$ 20", 2_000),
    ("Resgate voucher R$ 50", 5_000),
    ("Resgate frete grátis", 1_500),
    ("Resgate garrafa térmica", 8_000),
];

fn birth_date(rng: &mut Rng, birth: (u32, u32, u32)) -> Option<String> {
    match rng.below(100) {
        0..=69 => Some(date_iso(birth)),
        70..=94 => Some(date_br(birth)),
        95..=96 => rng.pick(IMPOSSIBLE_DATES).map(|d| (*d).to_owned()),
        _ => None,
    }
}

/// The last four digits of a linked card behind a mask, in the short or the
/// four-group layout; absent when no card is linked.
fn masked_card(rng: &mut Rng) -> Option<String> {
    let last_four = rng.below(10_000);
    match rng.below(100) {
        0..=44 => None,
        45..=89 => Some(format!("**** {last_four:04}")),
        _ => Some(format!("**** **** **** {last_four:04}")),
    }
}

/// One ledger entry's kind, signed points and description.
fn entry(rng: &mut Rng) -> (&'static str, i64, String) {
    let kind = weighted(rng, ENTRY_KINDS);
    match kind {
        "resgate" => {
            let (label, cost) = VOUCHERS[rng.index(VOUCHERS.len())];
            (kind, -cost, label.to_owned())
        }
        "expiracao" => (
            kind,
            -(rng.between(1, 40) as i64) * 50,
            "Expiração de pontos".to_owned(),
        ),
        "ajuste" => {
            let points = rng.between(1, 20) as i64 * 100;
            let signed = if rng.chance(700) { points } else { -points };
            (kind, signed, "Ajuste manual - atendimento".to_owned())
        }
        _ => {
            // Most purchases are small; a squared draw stretches the tail.
            let u = rng.between(1, 60);
            (
                kind,
                (u * u) as i64 + 10,
                format!("Compra loja {:04}", rng.between(1, 180)),
            )
        }
    }
}

pub fn generate(seed: u64, raw: &serde_json::Value) -> Result<DatasetDto, GenError> {
    let p: FidelidadeParams = params(raw)?;
    let root = Rng::new(seed);

    let mut membros = TableBuilder::new("membros")
        .primary_key("id", WireType::Integer)
        .column("nome", WireType::String)
        .column("cpf", WireType::String)
        .column("email", WireType::String)
        .column("telefone", WireType::String)
        .column("data_nascimento", WireType::String)
        .column("cartao_vinculado", WireType::String)
        .column("nivel", WireType::String)
        .column("data_adesao", WireType::String);
    let mut rng = root.fork("membros");
    let mut entries_per_member = Vec::with_capacity(p.membros);
    for index in 0..p.membros {
        let person = Person::generate(&mut rng);
        let cpf = (!rng.chance(50)).then(|| cpf_in_either_layout(&mut rng, &person.cpf, 620));
        membros.push(vec![
            Value::Int(index as i64 + 1),
            text(person.full_name.clone()),
            maybe_text(cpf),
            text(person.email.clone()),
            text(phone_masked(&person.phone)),
            maybe_text(birth_date(&mut rng, person.birth)),
            maybe_text(masked_card(&mut rng)),
            text(weighted(&mut rng, TIERS)),
            text(date_iso(day(&mut rng, 2016, 2024))),
        ])?;
        let rank = rng.between(1, 1000);
        entries_per_member.push(1 + rng.below(3) + p.cauda_lancamentos / rank);
    }

    let mut ledger_rng = root.fork("lancamentos");
    let mut owners: Vec<i64> = entries_per_member
        .iter()
        .enumerate()
        .flat_map(|(index, count)| std::iter::repeat_n(index as i64 + 1, *count as usize))
        .collect();
    for i in (1..owners.len()).rev() {
        let j = ledger_rng.index(i + 1);
        owners.swap(i, j);
    }
    let mut lancamentos = TableBuilder::new("lancamentos")
        .primary_key("id", WireType::Integer)
        .foreign_key("membro_id", WireType::Integer, "membros", "id")
        .column("data", WireType::String)
        .column("tipo", WireType::String)
        .column("pontos", WireType::Integer)
        .column("descricao", WireType::String);
    for member in owners {
        let (kind, points, description) = entry(&mut ledger_rng);
        lancamentos.push(vec![
            Value::Int(lancamentos.len() as i64 + 1),
            Value::Int(member),
            text(date_iso(day(&mut ledger_rng, 2022, 2024))),
            text(kind),
            Value::Int(points),
            text(description),
        ])?;
    }

    Ok(DatasetDto {
        tables: vec![membros.build(), lancamentos.build()],
    })
}
