//! A motor insurer: policyholders with CPF and driving licence, their
//! vehicles (plates in the old and the Mercosul layouts, RENAVAM stored as
//! an integer, chassis numbers), policies and claims whose narrative names
//! the third party involved.

use anonymizer::Value;
use serde::Deserialize;
use server::http::dto::{DatasetDto, WireType};

use super::ids::{
    chassis, plate_converted, plate_mercosul, plate_old, plate_old_hyphenated, renavam_generate,
    renavam_is_valid,
};
use super::words::{CLAIM_KINDS, CLAIM_STATUS, COVERAGES, VEHICLE_COLOURS, VEHICLE_MODELS};
use super::{date_between, heavy_tailed_cents};
use crate::generate::docs::{Document, text_of};
use crate::generate::mess::date_iso;
use crate::generate::person::{Person, cep_masked, phone_masked};
use crate::generate::rng::Rng;
use crate::generate::table::{TableBuilder, maybe_text, text};
use crate::generate::{GenError, params};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SegurosParams {
    segurados: usize,
    veiculos_max: u64,
    sinistros: usize,
}

/// A plate as the policy system holds it: old layout with or without the
/// hyphen, a Mercosul plate, or an old plate converted to Mercosul.
fn plate(rng: &mut Rng) -> String {
    match rng
        .weighted(&[(0u8, 30), (1, 15), (2, 40), (3, 15)])
        .copied()
    {
        Some(0) => plate_old_hyphenated(&plate_old(rng)),
        Some(1) => plate_old(rng),
        Some(3) => plate_converted(&plate_old(rng)),
        _ => plate_mercosul(rng),
    }
}

/// An eleven-digit licence number that is not also a valid CPF.
fn cnh(rng: &mut Rng) -> String {
    loop {
        let digits: Vec<u8> = (0..11)
            .map(|i| {
                if i == 0 {
                    1 + rng.below(9) as u8
                } else {
                    rng.below(10) as u8
                }
            })
            .collect();
        let candidate = text_of(&digits);
        if !Document::Cpf.is_valid(&candidate) {
            return candidate;
        }
    }
}

/// A RENAVAM as an integer column holds it, leading zeros gone.
fn renavam_integer(rng: &mut Rng) -> Result<i64, GenError> {
    let digits = renavam_generate(rng);
    let text = text_of(&digits);
    if !renavam_is_valid(&text) {
        return Err(GenError(format!("generated RENAVAM {text} is invalid")));
    }
    text.parse::<i64>()
        .map_err(|e| GenError(format!("RENAVAM {text}: {e}")))
}

fn claim_narrative(
    rng: &mut Rng,
    kind: &str,
    third_plate: Option<&str>,
    third_name: Option<&str>,
) -> String {
    match (kind, third_plate, third_name) {
        ("colisao" | "terceiros", Some(plate), Some(name)) => match rng.below(3) {
            0 => format!(
                "Colisão traseira no semáforo. Terceiro {name}, veículo placa {plate}, assumiu a culpa."
            ),
            1 => format!(
                "Segurado bateu no veículo {plate} de {name} ao manobrar no estacionamento."
            ),
            _ => format!("Abalroamento lateral com {plate}. Condutor do outro veículo: {name}."),
        },
        ("colisao", _, _) => "Colisão contra poste em via molhada, sem terceiros.".to_owned(),
        ("roubo", _, _) => "Veículo roubado à mão armada na saída do trabalho.".to_owned(),
        ("furto", _, _) => "Veículo furtado em via pública durante a madrugada.".to_owned(),
        ("vidros", _, _) => "Para-brisa trincado por pedra na rodovia.".to_owned(),
        ("alagamento", _, _) => "Veículo atingido por enchente na garagem.".to_owned(),
        _ => "Incêndio no compartimento do motor.".to_owned(),
    }
}

pub fn generate(seed: u64, raw: &serde_json::Value) -> Result<DatasetDto, GenError> {
    let p: SegurosParams = params(raw)?;
    let root = Rng::new(seed);

    let mut segurados = TableBuilder::new("segurados")
        .primary_key("id", WireType::Integer)
        .column("nome", WireType::String)
        .column("cpf", WireType::String)
        .column("numero_cnh", WireType::String)
        .column("data_nascimento", WireType::String)
        .column("telefone", WireType::String)
        .column("email", WireType::String)
        .column("cep", WireType::String);
    let mut veiculos = TableBuilder::new("veiculos")
        .primary_key("id", WireType::Integer)
        .foreign_key("segurado_id", WireType::Integer, "segurados", "id")
        .column("placa", WireType::String)
        .column("renavam", WireType::Integer)
        .column("chassi", WireType::String)
        .column("modelo", WireType::String)
        .column("ano_fabricacao", WireType::Integer)
        .column("cor", WireType::String);
    let mut apolices = TableBuilder::new("apolices")
        .primary_key("id", WireType::Integer)
        .foreign_key("veiculo_id", WireType::Integer, "veiculos", "id")
        .column("numero_apolice", WireType::String)
        .column("inicio_vigencia", WireType::String)
        .column("cobertura", WireType::String)
        .column("premio", WireType::Float)
        .column("franquia", WireType::Float);

    let mut rng = root.fork("segurados");
    let mut vehicle_rng = root.fork("veiculos");
    for index in 0..p.segurados {
        let person = Person::generate(&mut rng);
        let id = index as i64 + 1;
        segurados.push(vec![
            Value::Int(id),
            text(person.full_name.clone()),
            text(Document::Cpf.masked(&person.cpf)),
            maybe_text((!rng.chance(60)).then(|| cnh(&mut rng))),
            text(date_iso(person.birth)),
            text(phone_masked(&person.phone)),
            text(person.email.clone()),
            text(cep_masked(&person.cep)),
        ])?;
        for _ in 0..vehicle_rng.between(1, p.veiculos_max) {
            let vehicle_id = veiculos.len() as i64 + 1;
            let year = vehicle_rng.between(2008, 2024);
            veiculos.push(vec![
                Value::Int(vehicle_id),
                Value::Int(id),
                text(plate(&mut vehicle_rng)),
                Value::Int(renavam_integer(&mut vehicle_rng)?),
                text(chassis(&mut vehicle_rng)),
                text(
                    vehicle_rng
                        .weighted(VEHICLE_MODELS)
                        .copied()
                        .unwrap_or("Hatch 1.6 Flex"),
                ),
                Value::Int(year as i64),
                text(
                    vehicle_rng
                        .weighted(VEHICLE_COLOURS)
                        .copied()
                        .unwrap_or("Prata"),
                ),
            ])?;
            let start = date_between(&mut vehicle_rng, 2023, 2024);
            let premium = vehicle_rng.between(1_200, 9_500) * 100 + vehicle_rng.below(100);
            apolices.push(vec![
                Value::Int(vehicle_id),
                Value::Int(vehicle_id),
                text(format!("AP-{}-{:06}", start.0, vehicle_id * 13)),
                text(date_iso(start)),
                text(
                    vehicle_rng
                        .weighted(COVERAGES)
                        .copied()
                        .unwrap_or("Compreensiva"),
                ),
                Value::Float(premium as f64 / 100.0),
                Value::Float((premium / 2 / 100 * 100) as f64 / 100.0),
            ])?;
        }
    }

    let mut sinistros = TableBuilder::new("sinistros")
        .primary_key("id", WireType::Integer)
        .foreign_key("apolice_id", WireType::Integer, "apolices", "id")
        .column("data_ocorrencia", WireType::String)
        .column("tipo", WireType::String)
        .column("status", WireType::String)
        .column("valor_indenizacao", WireType::Float)
        .column("boletim_ocorrencia", WireType::String)
        .column("placa_terceiro", WireType::String)
        .column("nome_terceiro", WireType::String)
        .column("telefone_terceiro", WireType::String)
        .column("descricao", WireType::String);
    let mut claim_rng = root.fork("sinistros");
    let mut third_rng = root.fork("terceiros");
    let policies = apolices.len() as u64;
    for index in 0..p.sinistros {
        let kind = claim_rng
            .weighted(CLAIM_KINDS)
            .copied()
            .unwrap_or("colisao");
        let third = (matches!(kind, "colisao" | "terceiros") && claim_rng.chance(650))
            .then(|| (plate(&mut third_rng), Person::generate(&mut third_rng)));
        let amount = match kind {
            "vidros" => claim_rng.between(400, 3_500) * 100,
            "roubo" | "furto" | "incendio" | "alagamento" => {
                claim_rng.between(25_000, 140_000) * 100
            }
            _ => heavy_tailed_cents(
                &mut claim_rng,
                &[(100_000, 55), (1_000_000, 40), (10_000_000, 5)],
            ),
        };
        let police_report = matches!(kind, "roubo" | "furto" | "terceiros" | "colisao")
            .then(|| claim_rng.chance(700))
            .filter(|reported| *reported)
            .map(|_| {
                format!(
                    "BO {}/2024 - {}º DP",
                    claim_rng.between(100, 99_999),
                    claim_rng.between(1, 99)
                )
            });
        let narrative = claim_narrative(
            &mut claim_rng,
            kind,
            third.as_ref().map(|(plate, _)| plate.as_str()),
            third.as_ref().map(|(_, person)| person.full_name.as_str()),
        );
        sinistros.push(vec![
            Value::Int(index as i64 + 1),
            Value::Int(claim_rng.between(1, policies) as i64),
            text(date_iso(date_between(&mut claim_rng, 2023, 2024))),
            text(kind),
            text(
                claim_rng
                    .weighted(CLAIM_STATUS)
                    .copied()
                    .unwrap_or("em_analise"),
            ),
            Value::Float(amount as f64 / 100.0),
            maybe_text(police_report),
            maybe_text(third.as_ref().map(|(plate, _)| plate.clone())),
            maybe_text(third.as_ref().map(|(_, person)| person.full_name.clone())),
            maybe_text(
                third
                    .as_ref()
                    .map(|(_, person)| phone_masked(&person.phone)),
            ),
            text(narrative),
        ])?;
    }

    Ok(DatasetDto {
        tables: vec![
            segurados.build(),
            veiculos.build(),
            apolices.build(),
            sinistros.build(),
        ],
    })
}
