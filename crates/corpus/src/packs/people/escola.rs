//! A school's enrolment system: guardians, the minors in their care (most
//! without a CPF), classes, enrolments and bimonthly grades. Siblings share
//! a guardian and a surname; a few guardian rows are test placeholders.

use anonymizer::Value;
use serde::Deserialize;
use server::http::dto::{DatasetDto, WireType};

use super::date_between;
use super::words::{DISCIPLINAS, PARENTESCO_RESPONSAVEL, SERIES};
use crate::generate::docs::{Document, text_of};
use crate::generate::mess::{
    PLACEHOLDER_CPF, PLACEHOLDER_EMAIL, PLACEHOLDER_NAME, PLACEHOLDER_PHONE, date_iso,
};
use crate::generate::person::{Person, phone_masked};
use crate::generate::rng::Rng;
use crate::generate::table::{TableBuilder, maybe_text, text};
use crate::generate::vocab::{FIRST_NAMES_FEMALE, FIRST_NAMES_MALE};
use crate::generate::{GenError, params};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EscolaParams {
    responsaveis: usize,
    placeholders: usize,
    filhos_max: u64,
    disciplinas: usize,
    bimestres: u64,
}

const NEEDS: &[(&str, u64)] = &[
    ("TDAH", 40),
    ("TEA", 25),
    ("Dislexia", 18),
    ("Baixa visão", 8),
    ("Deficiência auditiva", 6),
    ("Deficiência física", 3),
];

const TURNOS: &[&str] = &["Manhã", "Tarde"];

const SITUACOES: &[(&str, u64)] = &[
    ("ativa", 90),
    ("transferida", 5),
    ("trancada", 2),
    ("concluida", 3),
];

/// A student registration number as state systems print it: twelve
/// digits, a hyphen and a check character.
fn registro_aluno(rng: &mut Rng, sequence: usize) -> String {
    let check = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "X"][rng.index(11)];
    format!("{:012}-{check}", 100_000_000 + sequence as u64 * 37)
}

/// A grade from 0 to 10 with one decimal, centred on a pupil's level.
fn grade(rng: &mut Rng, level: u64) -> f64 {
    let tenths = (level * 10 + rng.between(0, 40))
        .saturating_sub(20)
        .min(100);
    tenths as f64 / 10.0
}

pub fn generate(seed: u64, raw: &serde_json::Value) -> Result<DatasetDto, GenError> {
    let p: EscolaParams = params(raw)?;
    let root = Rng::new(seed);

    let mut turmas = TableBuilder::new("turmas")
        .primary_key("id", WireType::Integer)
        .column("serie", WireType::String)
        .column("turno", WireType::String)
        .column("ano_letivo", WireType::Integer);
    for (index, serie) in SERIES.iter().enumerate() {
        for (shift, turno) in TURNOS.iter().enumerate() {
            turmas.push(vec![
                Value::Int((index * TURNOS.len() + shift) as i64 + 1),
                text(*serie),
                text(*turno),
                Value::Int(2024),
            ])?;
        }
    }

    let mut responsaveis = TableBuilder::new("responsaveis")
        .primary_key("id", WireType::Integer)
        .column("nome", WireType::String)
        .column("cpf", WireType::String)
        .column("telefone", WireType::String)
        .column("email", WireType::String);
    let mut alunos = TableBuilder::new("alunos")
        .primary_key("id", WireType::Integer)
        .column("registro_aluno", WireType::String)
        .column("nome", WireType::String)
        .column("cpf", WireType::String)
        .column("data_nascimento", WireType::String)
        .column("sexo", WireType::String)
        .foreign_key("responsavel_id", WireType::Integer, "responsaveis", "id")
        .column("parentesco", WireType::String)
        .foreign_key(
            "responsavel_secundario_id",
            WireType::Integer,
            "responsaveis",
            "id",
        )
        .column("necessidade_especial", WireType::String);
    let mut matriculas = TableBuilder::new("matriculas")
        .primary_key("id", WireType::Integer)
        .foreign_key("aluno_id", WireType::Integer, "alunos", "id")
        .foreign_key("turma_id", WireType::Integer, "turmas", "id")
        .column("data_matricula", WireType::String)
        .column("situacao", WireType::String);
    let mut notas = TableBuilder::new("notas")
        .primary_key("id", WireType::Integer)
        .foreign_key("matricula_id", WireType::Integer, "matriculas", "id")
        .column("disciplina", WireType::String)
        .column("bimestre", WireType::Integer)
        .column("nota", WireType::Float)
        .column("faltas", WireType::Integer);

    let mut rng = root.fork("responsaveis");
    let mut child_rng = root.fork("alunos");
    let mut grade_rng = root.fork("notas");
    let mut placeholder_rows = Vec::new();
    let mut placeholder_rng = root.fork("placeholders");
    while placeholder_rows.len() < p.placeholders.min(p.responsaveis) {
        let row = placeholder_rng.index(p.responsaveis);
        if !placeholder_rows.contains(&row) {
            placeholder_rows.push(row);
        }
    }
    let subjects = &DISCIPLINAS[..p.disciplinas.min(DISCIPLINAS.len())];
    for index in 0..p.responsaveis {
        let guardian = Person::generate(&mut rng);
        let id = index as i64 + 1;
        if placeholder_rows.contains(&index) {
            responsaveis.push(vec![
                Value::Int(id),
                text(PLACEHOLDER_NAME),
                text(PLACEHOLDER_CPF),
                text(PLACEHOLDER_PHONE),
                text(PLACEHOLDER_EMAIL),
            ])?;
        } else {
            responsaveis.push(vec![
                Value::Int(id),
                text(guardian.full_name.clone()),
                maybe_text((!rng.chance(40)).then(|| Document::Cpf.masked(&guardian.cpf))),
                text(phone_masked(&guardian.phone)),
                maybe_text((!rng.chance(120)).then(|| guardian.email.clone())),
            ])?;
        }
        let surname = guardian.full_name.rsplit(' ').next().unwrap_or("Silva");
        let kinship = rng
            .weighted(PARENTESCO_RESPONSAVEL)
            .copied()
            .unwrap_or("Mãe");
        for _ in 0..child_rng.between(1, p.filhos_max) {
            let student_id = alunos.len() as i64 + 1;
            let female = child_rng.chance(500);
            let pool = if female {
                FIRST_NAMES_FEMALE
            } else {
                FIRST_NAMES_MALE
            };
            let first = child_rng.pick(pool).copied().unwrap_or("Ana");
            let born = date_between(&mut child_rng, 2007, 2019);
            let cpf_digits = Document::Cpf.generate(&mut child_rng);
            let cpf = match child_rng.below(100) {
                0..=77 => None,
                78..=91 => Some(Document::Cpf.masked(&cpf_digits)),
                _ => Some(text_of(&cpf_digits)),
            };
            let second =
                (child_rng.chance(350)).then(|| child_rng.between(1, p.responsaveis as u64));
            let need = child_rng
                .chance(90)
                .then(|| child_rng.weighted(NEEDS).map(|n| (*n).to_owned()))
                .flatten();
            let serie = (2024 - born.0 as usize)
                .saturating_sub(6)
                .min(SERIES.len() - 1);
            let turma = serie * TURNOS.len() + child_rng.index(TURNOS.len()) + 1;
            alunos.push(vec![
                Value::Int(student_id),
                text(registro_aluno(&mut child_rng, alunos.len())),
                text(format!("{first} {surname}")),
                maybe_text(cpf),
                text(date_iso(born)),
                text(if female { "F" } else { "M" }),
                Value::Int(id),
                text(kinship),
                second.map_or(Value::Null, |s| Value::Int(s as i64)),
                maybe_text(need),
            ])?;
            let enrolment_id = matriculas.len() as i64 + 1;
            matriculas.push(vec![
                Value::Int(enrolment_id),
                Value::Int(student_id),
                Value::Int(turma as i64),
                text(date_iso(date_between(&mut child_rng, 2023, 2023))),
                text(child_rng.weighted(SITUACOES).copied().unwrap_or("ativa")),
            ])?;
            let level = grade_rng.between(4, 9);
            for subject in subjects {
                for bimestre in 1..=p.bimestres {
                    notas.push(vec![
                        Value::Int(notas.len() as i64 + 1),
                        Value::Int(enrolment_id),
                        text(*subject),
                        Value::Int(bimestre as i64),
                        Value::Float(grade(&mut grade_rng, level)),
                        Value::Int(
                            grade_rng
                                .weighted(&[(0i64, 50), (1, 20), (2, 12), (4, 10), (8, 6), (15, 2)])
                                .copied()
                                .unwrap_or(0),
                        ),
                    ])?;
                }
            }
        }
    }

    Ok(DatasetDto {
        tables: vec![
            turmas.build(),
            responsaveis.build(),
            alunos.build(),
            matriculas.build(),
            notas.build(),
        ],
    })
}
