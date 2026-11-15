//! A clinic's electronic records: patients identified by CNS and often
//! without a CPF, the clinic's doctors, and consultations coded in CID-10,
//! with a patient history field that discloses sensitive health data and
//! repeats the patient's own identifiers.

use anonymizer::Value;
use serde::Deserialize;
use server::http::dto::{DatasetDto, WireType};

use super::date_between;
use super::ids::{cns_generate, cns_is_valid, cns_spaced};
use super::words::{
    CID10, COMPLAINTS, CONDUCTS, CONVENIOS, ESPECIALIDADES, SENSITIVE_FINDINGS, TIPOS_SANGUINEOS,
};
use crate::generate::docs::{Document, text_of};
use crate::generate::mess::{date_br, date_iso};
use crate::generate::person::{Person, phone_masked};
use crate::generate::rng::Rng;
use crate::generate::table::{TableBuilder, maybe_text, text};
use crate::generate::vocab::STATES;
use crate::generate::{GenError, params};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClinicaParams {
    pacientes: usize,
    medicos: usize,
    consultas: usize,
}

/// The CNS column's mix: spaced as on the card, bare digits, null, and a
/// number with a wrong last digit.
fn messy_cns(rng: &mut Rng, cns: &[u8]) -> Option<String> {
    match rng.weighted(&[(0u8, 55), (1, 35), (2, 6), (3, 4)])? {
        0 => Some(cns_spaced(cns)),
        1 => Some(text_of(cns)),
        2 => None,
        _ => {
            let mut wrong = cns.to_vec();
            loop {
                wrong[14] = (wrong[14] + 1 + rng.below(9) as u8) % 10;
                if !cns_is_valid(&text_of(&wrong)) {
                    return Some(cns_spaced(&wrong));
                }
            }
        }
    }
}

/// A history note as a receptionist and a doctor type it: the patient's
/// first name, sometimes a contact phone or the CNS, a complaint, and in
/// a share of rows a sensitive finding.
fn history_note(rng: &mut Rng, first_name: &str, phone: &str, cns: &[u8]) -> Option<String> {
    let mut note = String::new();
    match rng.below(10) {
        0..=2 => return None,
        3..=4 => note.push_str(&format!("{first_name} refere ")),
        5 => note.push_str(&format!("Paciente {first_name}, contato {phone}. Refere ")),
        6 => note.push_str(&format!("CNS {} conferido. Refere ", cns_spaced(cns))),
        _ => note.push_str("Paciente refere "),
    }
    note.push_str(rng.pick(COMPLAINTS).copied().unwrap_or("cefaleia"));
    note.push_str(". ");
    if rng.chance(300) {
        note.push_str(rng.pick(SENSITIVE_FINDINGS).copied().unwrap_or(""));
        note.push(' ');
    }
    note.push_str(rng.pick(CONDUCTS).copied().unwrap_or("Retorno em 30 dias."));
    Some(note)
}

pub fn generate(seed: u64, raw: &serde_json::Value) -> Result<DatasetDto, GenError> {
    let p: ClinicaParams = params(raw)?;
    let root = Rng::new(seed);

    let mut pacientes = TableBuilder::new("pacientes")
        .primary_key("id", WireType::Integer)
        .column("nome", WireType::String)
        .column("cpf", WireType::String)
        .column("cns", WireType::String)
        .column("data_nascimento", WireType::String)
        .column("sexo", WireType::String)
        .column("tipo_sanguineo", WireType::String)
        .column("telefone", WireType::String)
        .column("email", WireType::String)
        .column("convenio", WireType::String)
        .column("historico", WireType::String);
    let mut rng = root.fork("pacientes");
    for index in 0..p.pacientes {
        let person = Person::generate(&mut rng);
        let cns = cns_generate(&mut rng);
        let born = date_between(&mut rng, 1935, 2023);
        let cpf = match rng.below(100) {
            0..=54 => Some(Document::Cpf.masked(&person.cpf)),
            55..=79 => Some(text_of(&person.cpf)),
            _ => None,
        };
        let female = rng.chance(560);
        let note = history_note(
            &mut rng,
            &person.first_name,
            &phone_masked(&person.phone),
            &cns,
        );
        pacientes.push(vec![
            Value::Int(index as i64 + 1),
            text(person.full_name.clone()),
            maybe_text(cpf),
            maybe_text(messy_cns(&mut rng, &cns)),
            text(if rng.chance(850) {
                date_iso(born)
            } else {
                date_br(born)
            }),
            text(if female { "F" } else { "M" }),
            maybe_text(
                (!rng.chance(250))
                    .then(|| rng.weighted(TIPOS_SANGUINEOS).map(|t| (*t).to_owned()))
                    .flatten(),
            ),
            text(phone_masked(&person.phone)),
            maybe_text((!rng.chance(300)).then(|| person.email.clone())),
            text(rng.weighted(CONVENIOS).copied().unwrap_or("SUS")),
            maybe_text(note),
        ])?;
    }

    let mut medicos = TableBuilder::new("medicos")
        .primary_key("id", WireType::Integer)
        .column("nome", WireType::String)
        .column("crm", WireType::String)
        .column("especialidade", WireType::String);
    let mut doctor_rng = root.fork("medicos");
    for index in 0..p.medicos {
        let person = Person::generate(&mut doctor_rng);
        let uf = if doctor_rng.chance(800) {
            person.city.uf
        } else {
            doctor_rng.pick(STATES).map_or("SP", |s| s.0)
        };
        medicos.push(vec![
            Value::Int(index as i64 + 1),
            text(format!("Dr(a). {}", person.full_name)),
            text(format!("CRM/{uf} {}", doctor_rng.between(10_000, 250_000))),
            text(
                doctor_rng
                    .weighted(ESPECIALIDADES)
                    .copied()
                    .unwrap_or("Clínica Geral"),
            ),
        ])?;
    }

    let mut consultas = TableBuilder::new("consultas")
        .primary_key("id", WireType::Integer)
        .foreign_key("paciente_id", WireType::Integer, "pacientes", "id")
        .foreign_key("medico_id", WireType::Integer, "medicos", "id")
        .column("data_hora", WireType::String)
        .column("cid10", WireType::String)
        .column("retorno", WireType::Boolean)
        .column("valor", WireType::Float);
    let mut visit_rng = root.fork("consultas");
    let patients = p.pacientes as u64;
    for index in 0..p.consultas {
        let u = visit_rng.below(patients);
        let patient = u * u / patients.max(1) + 1;
        let date = date_between(&mut visit_rng, 2022, 2024);
        let price = match visit_rng.below(10) {
            0..=2 => 0,
            3..=7 => visit_rng.between(150, 400) * 100,
            _ => visit_rng.between(400, 900) * 100,
        };
        consultas.push(vec![
            Value::Int(index as i64 + 1),
            Value::Int(patient as i64),
            Value::Int(visit_rng.between(1, p.medicos as u64) as i64),
            text(format!(
                "{} {:02}:{:02}",
                date_iso(date),
                visit_rng.between(7, 18),
                [0, 15, 30, 45][visit_rng.index(4)]
            )),
            text(visit_rng.weighted(CID10).copied().unwrap_or("Z00.0")),
            Value::Bool(visit_rng.chance(300)),
            Value::Float(price as f64 / 100.0),
        ])?;
    }

    Ok(DatasetDto {
        tables: vec![pacientes.build(), medicos.build(), consultas.build()],
    })
}
