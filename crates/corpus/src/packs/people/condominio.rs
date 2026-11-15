//! A residential condominium's management system: units, residents with
//! the number the intercom dials, residents' vehicles, and a gatehouse log
//! typed by the doorman in free form.

use anonymizer::Value;
use serde::Deserialize;
use server::http::dto::{DatasetDto, WireType};

use super::date_between;
use super::ids::{plate_mercosul, plate_old, plate_old_hyphenated, rg_for_state};
use super::words::{VEHICLE_COLOURS, VEHICLE_MODELS, VISIT_REASONS};
use crate::generate::docs::Document;
use crate::generate::mess::{date_br, date_iso};
use crate::generate::person::{Person, phone_masked};
use crate::generate::rng::Rng;
use crate::generate::table::{TableBuilder, maybe_text, text};
use crate::generate::{GenError, params};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CondominioParams {
    blocos: usize,
    andares: u64,
    apartamentos_por_andar: u64,
    visitas: usize,
}

const BLOCOS: &[&str] = &["A", "B", "C", "D", "E", "F"];

const RESIDENT_KINDS: &[(&str, u64)] =
    &[("proprietario", 45), ("inquilino", 25), ("dependente", 30)];

/// What the intercom dials: the resident's mobile, a landline, or the
/// unit's internal extension.
fn intercom(rng: &mut Rng, person: &Person, extension: &str) -> String {
    match rng.below(10) {
        0..=6 => phone_masked(&person.phone),
        7 => person.phone.clone(),
        _ => format!("ramal {extension}"),
    }
}

/// A visitor's document as the doorman writes it: an RG or a CPF.
fn visitor_document(rng: &mut Rng, visitor: &Person) -> String {
    if rng.chance(650) {
        rg_for_state(rng, "SP")
    } else {
        Document::Cpf.masked(&visitor.cpf)
    }
}

fn gatehouse_entry(
    rng: &mut Rng,
    name: &str,
    document: &str,
    plate: Option<&str>,
    unit: &str,
    reason: &str,
    phone: &str,
) -> String {
    let car = plate.map_or_else(String::new, |p| format!(" carro {p}"));
    match rng.below(5) {
        0 => format!("{name} RG/CPF {document} p/ apto {unit} - {reason}{car}"),
        1 => format!("Visitante {name}, doc {document}, autorizado pelo morador do {unit}.{car}"),
        2 => format!("{reason} para o {unit}: {name} tel {phone}{car}"),
        3 => format!("entrada {name} ({document}) {unit}{car} liberado interfone"),
        _ => format!("{} - {unit} - {reason}{car}", name.to_uppercase()),
    }
}

pub fn generate(seed: u64, raw: &serde_json::Value) -> Result<DatasetDto, GenError> {
    let p: CondominioParams = params(raw)?;
    let root = Rng::new(seed);

    let mut unidades = TableBuilder::new("unidades")
        .primary_key("id", WireType::Integer)
        .column("bloco", WireType::String)
        .column("numero", WireType::String)
        .column("ramal", WireType::String)
        .column("area_m2", WireType::Float)
        .column("vagas", WireType::Integer);
    let mut moradores = TableBuilder::new("moradores")
        .primary_key("id", WireType::Integer)
        .foreign_key("unidade_id", WireType::Integer, "unidades", "id")
        .column("nome", WireType::String)
        .column("cpf", WireType::String)
        .column("rg", WireType::String)
        .column("telefone_interfone", WireType::String)
        .column("email", WireType::String)
        .column("tipo", WireType::String)
        .column("data_entrada", WireType::String);
    let mut veiculos = TableBuilder::new("veiculos")
        .primary_key("id", WireType::Integer)
        .foreign_key("unidade_id", WireType::Integer, "unidades", "id")
        .column("placa", WireType::String)
        .column("modelo", WireType::String)
        .column("cor", WireType::String)
        .column("vaga", WireType::String);

    let mut rng = root.fork("unidades");
    let mut resident_rng = root.fork("moradores");
    let mut car_rng = root.fork("veiculos");
    let mut unit_labels = Vec::new();
    for bloco in BLOCOS.iter().take(p.blocos) {
        for andar in 1..=p.andares {
            for apto in 1..=p.apartamentos_por_andar {
                let id = unidades.len() as i64 + 1;
                let numero = format!("{andar}{apto:02}");
                let extension = format!("{}{numero:0>4}", bloco.to_lowercase());
                let parking = if apto == 1 { 2 } else { 1 };
                let area = if apto == 1 { 98 } else { 64 } * 100 + rng.below(300);
                unidades.push(vec![
                    Value::Int(id),
                    text(*bloco),
                    text(numero.clone()),
                    text(extension.clone()),
                    Value::Float(area as f64 / 100.0),
                    Value::Int(parking),
                ])?;
                unit_labels.push(format!("{bloco}{numero}"));
                if resident_rng.chance(60) {
                    continue;
                }
                for resident in 0..resident_rng.between(1, 4) {
                    let person = Person::generate(&mut resident_rng);
                    let kind = if resident == 0 {
                        if resident_rng.chance(650) {
                            "proprietario"
                        } else {
                            "inquilino"
                        }
                    } else {
                        resident_rng
                            .weighted(RESIDENT_KINDS)
                            .copied()
                            .unwrap_or("dependente")
                    };
                    let minor = kind == "dependente" && resident_rng.chance(500);
                    moradores.push(vec![
                        Value::Int(moradores.len() as i64 + 1),
                        Value::Int(id),
                        text(person.full_name.clone()),
                        maybe_text((!minor).then(|| Document::Cpf.masked(&person.cpf))),
                        maybe_text(
                            (!minor && resident_rng.chance(700))
                                .then(|| rg_for_state(&mut resident_rng, person.city.uf)),
                        ),
                        maybe_text(
                            (!minor).then(|| intercom(&mut resident_rng, &person, &extension)),
                        ),
                        maybe_text(
                            (!minor && resident_rng.chance(800)).then(|| person.email.clone()),
                        ),
                        text(kind),
                        text(date_iso(date_between(&mut resident_rng, 2005, 2024))),
                    ])?;
                }
                for slot in 0..car_rng.below(parking as u64 + 1) {
                    let plate = if car_rng.chance(500) {
                        plate_mercosul(&mut car_rng)
                    } else if car_rng.chance(500) {
                        plate_old_hyphenated(&plate_old(&mut car_rng))
                    } else {
                        plate_old(&mut car_rng)
                    };
                    veiculos.push(vec![
                        Value::Int(veiculos.len() as i64 + 1),
                        Value::Int(id),
                        text(plate),
                        text(
                            car_rng
                                .weighted(VEHICLE_MODELS)
                                .copied()
                                .unwrap_or("Hatch 1.6 Flex"),
                        ),
                        text(
                            car_rng
                                .weighted(VEHICLE_COLOURS)
                                .copied()
                                .unwrap_or("Prata"),
                        ),
                        text(format!("G{}-{:03}", 1 + slot, id * 2 + slot as i64)),
                    ])?;
                }
            }
        }
    }

    let mut visitas = TableBuilder::new("visitas")
        .primary_key("id", WireType::Integer)
        .foreign_key("unidade_id", WireType::Integer, "unidades", "id")
        .column("entrada", WireType::String)
        .column("nome_visitante", WireType::String)
        .column("documento_visitante", WireType::String)
        .column("placa_visitante", WireType::String)
        .column("motivo", WireType::String)
        .column("registro", WireType::String);
    let mut visit_rng = root.fork("visitas");
    let units = unit_labels.len();
    let mut regulars: Vec<(String, String)> = Vec::new();
    for index in 0..p.visitas {
        let unit = visit_rng.index(units);
        let visitor = Person::generate(&mut visit_rng);
        let (name, document) = if !regulars.is_empty() && visit_rng.chance(250) {
            regulars[visit_rng.index(regulars.len())].clone()
        } else {
            let entry = (
                visitor.full_name.clone(),
                visitor_document(&mut visit_rng, &visitor),
            );
            if regulars.len() < 40 {
                regulars.push(entry.clone());
            }
            entry
        };
        let plate = visit_rng.chance(300).then(|| {
            if visit_rng.chance(600) {
                plate_mercosul(&mut visit_rng)
            } else {
                plate_old(&mut visit_rng)
            }
        });
        let reason = visit_rng.pick(VISIT_REASONS).copied().unwrap_or("visita");
        let when = date_between(&mut visit_rng, 2024, 2024);
        let entrada = format!(
            "{} {:02}:{:02}",
            date_br(when),
            visit_rng.between(6, 23),
            visit_rng.below(60)
        );
        let registro = gatehouse_entry(
            &mut visit_rng,
            &name,
            &document,
            plate.as_deref(),
            &unit_labels[unit],
            reason,
            &phone_masked(&visitor.phone),
        );
        visitas.push(vec![
            Value::Int(index as i64 + 1),
            Value::Int(unit as i64 + 1),
            text(entrada),
            text(name),
            text(document),
            maybe_text(plate),
            text(reason),
            text(registro),
        ])?;
    }

    Ok(DatasetDto {
        tables: vec![
            unidades.build(),
            moradores.build(),
            veiculos.build(),
            visitas.build(),
        ],
    })
}
