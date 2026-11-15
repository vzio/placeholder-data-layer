//! A payroll database: a job-title lookup, employees with CPF, PIS/PASEP,
//! state RGs and bank details, their dependents, and three months of
//! payslips.

use anonymizer::Value;
use serde::Deserialize;
use server::http::dto::{DatasetDto, WireType};

use super::date_between;
use super::ids::{bank_account, bank_branch, pis_generate, pis_is_valid, pis_masked, rg_for_state};
use super::words::{BANK_CODES, CARGOS, DEPARTAMENTOS, PARENTESCOS};
use crate::generate::docs::{Document, text_of};
use crate::generate::mess::{date_br, date_iso};
use crate::generate::person::{Person, ascii_fold, phone_masked};
use crate::generate::rng::Rng;
use crate::generate::table::{TableBuilder, maybe_text, text};
use crate::generate::{GenError, params};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FolhaParams {
    funcionarios: usize,
    dependentes_max: u64,
    competencias: Vec<String>,
    dominio: String,
}

/// The PIS column's mix: masked, bare digits, wrong check digit, the
/// employee's CPF typed into the wrong field, and null.
fn messy_pis(rng: &mut Rng, pis: &[u8], cpf: &[u8]) -> Option<String> {
    match rng.weighted(&[(0u8, 58), (1, 30), (2, 4), (3, 3), (4, 5)])? {
        0 => Some(pis_masked(pis)),
        1 => Some(text_of(pis)),
        2 => {
            let mut wrong = pis.to_vec();
            loop {
                wrong[10] = (wrong[10] + 1 + rng.below(9) as u8) % 10;
                if !pis_is_valid(&text_of(&wrong)) && !Document::Cpf.is_valid(&text_of(&wrong)) {
                    return Some(pis_masked(&wrong));
                }
            }
        }
        3 => Some(text_of(cpf)),
        _ => None,
    }
}

/// A valid PIS that is not also a valid CPF by coincidence, so a PIS cell
/// that validates as a CPF is always one typed into the wrong field.
fn pis_that_is_no_cpf(rng: &mut Rng) -> Vec<u8> {
    loop {
        let pis = pis_generate(rng);
        if !Document::Cpf.is_valid(&text_of(&pis)) {
            return pis;
        }
    }
}

/// Monthly INSS contribution in cents, by the progressive brackets.
fn inss_cents(gross: u64) -> u64 {
    const BRACKETS: [(u64, u64); 4] =
        [(141_200, 75), (266_668, 90), (400_003, 120), (778_602, 140)];
    let mut owed = 0;
    let mut floor = 0;
    for (ceiling, rate_per_mille) in BRACKETS {
        if gross <= floor {
            break;
        }
        let taxed = gross.min(ceiling) - floor;
        owed += taxed * rate_per_mille / 1000;
        floor = ceiling;
    }
    owed
}

/// Monthly IRRF withholding in cents on the base after INSS.
fn irrf_cents(base: u64, dependents: u64) -> u64 {
    let base = base.saturating_sub(dependents * 18_959);
    let (rate, deduction) = match base {
        0..=225_920 => (0, 0),
        225_921..=282_665 => (75, 16_944),
        282_666..=375_105 => (150, 38_144),
        375_106..=466_468 => (225, 66_277),
        _ => (275, 89_600),
    };
    (base * rate / 1000).saturating_sub(deduction)
}

pub fn generate(seed: u64, raw: &serde_json::Value) -> Result<DatasetDto, GenError> {
    let p: FolhaParams = params(raw)?;
    let root = Rng::new(seed);

    let mut cargos = TableBuilder::new("cargos")
        .primary_key("id", WireType::Integer)
        .column("codigo_cbo", WireType::String)
        .column("titulo", WireType::String)
        .column("salario_minimo", WireType::Float)
        .column("salario_maximo", WireType::Float);
    for (index, (cbo, title, low, high, _)) in CARGOS.iter().enumerate() {
        cargos.push(vec![
            Value::Int(index as i64 + 1),
            text(*cbo),
            text(*title),
            Value::Float(*low as f64),
            Value::Float(*high as f64),
        ])?;
    }
    let staffing: Vec<(usize, u64)> = CARGOS.iter().enumerate().map(|(i, c)| (i, c.4)).collect();

    let mut funcionarios = TableBuilder::new("funcionarios")
        .primary_key("id", WireType::Integer)
        .column("matricula", WireType::String)
        .column("nome", WireType::String)
        .column("cpf", WireType::String)
        .column("pis_pasep", WireType::String)
        .column("rg", WireType::String)
        .column("rg_uf", WireType::String)
        .column("data_nascimento", WireType::String)
        .column("data_admissao", WireType::String)
        .column("data_demissao", WireType::String)
        .foreign_key("cargo_id", WireType::Integer, "cargos", "id")
        .column("departamento", WireType::String)
        .column("salario_base", WireType::Float)
        .column("banco", WireType::String)
        .column("agencia", WireType::String)
        .column("conta", WireType::String)
        .column("email_corporativo", WireType::String)
        .column("telefone", WireType::String);
    let mut dependentes = TableBuilder::new("dependentes")
        .primary_key("id", WireType::Integer)
        .foreign_key("funcionario_id", WireType::Integer, "funcionarios", "id")
        .column("nome", WireType::String)
        .column("cpf", WireType::String)
        .column("data_nascimento", WireType::String)
        .column("parentesco", WireType::String);

    let mut rng = root.fork("funcionarios");
    let mut dep_rng = root.fork("dependentes");
    let mut employees: Vec<(i64, u64, u64)> = Vec::with_capacity(p.funcionarios);
    for index in 0..p.funcionarios {
        let id = index as i64 + 1;
        let person = Person::generate(&mut rng);
        let cargo = rng.weighted(&staffing).copied().unwrap_or(0);
        let (_, _, low, high, _) = CARGOS[cargo];
        let salary_cents = rng.between(low * 100, high * 100) / 100 * 100;
        let pis = pis_that_is_no_cpf(&mut rng);
        let rg_uf = if rng.chance(85) {
            rng.pick(&["SP", "RJ", "MG", "BA", "PR"])
                .copied()
                .unwrap_or("SP")
        } else {
            person.city.uf
        };
        let birth = date_between(&mut rng, 1962, 2005);
        let hired = date_between(&mut rng, 2008.max(birth.0 + 18), 2024);
        let fired = rng
            .chance(120)
            .then(|| date_between(&mut rng, hired.0.max(2020), 2024));
        let cpf = match rng.below(100) {
            0..=69 => Document::Cpf.masked(&person.cpf),
            _ => text_of(&person.cpf),
        };
        let local = format!(
            "{}.{}",
            ascii_fold(&person.first_name).to_lowercase(),
            ascii_fold(person.full_name.rsplit(' ').next().unwrap_or("silva")).to_lowercase()
        );
        let dependents = dep_rng.below(p.dependentes_max + 1);
        funcionarios.push(vec![
            Value::Int(id),
            text(format!("F{:06}", 1000 + index * 7)),
            text(person.full_name.clone()),
            text(cpf),
            maybe_text(messy_pis(&mut rng, &pis, &person.cpf)),
            text(rg_for_state(&mut rng, rg_uf)),
            text(rg_uf),
            text(date_iso(birth)),
            text(if rng.chance(800) {
                date_iso(hired)
            } else {
                date_br(hired)
            }),
            maybe_text(fired.map(date_iso)),
            Value::Int(cargo as i64 + 1),
            text(rng.weighted(DEPARTAMENTOS).copied().unwrap_or("Produção")),
            Value::Float(salary_cents as f64 / 100.0),
            text(rng.weighted(BANK_CODES).copied().unwrap_or("001")),
            text(bank_branch(&mut rng)),
            text(bank_account(&mut rng)),
            text(format!("{local}@{}", p.dominio)),
            text(phone_masked(&person.phone)),
        ])?;
        let surname = person.full_name.rsplit(' ').next().unwrap_or("Silva");
        for _ in 0..dependents {
            let dependent = Person::generate(&mut dep_rng);
            let kinship = dep_rng.weighted(PARENTESCOS).copied().unwrap_or("Filho(a)");
            let child = kinship == "Filho(a)" || kinship == "Enteado(a)";
            let born = if child {
                date_between(&mut dep_rng, birth.0 + 18, 2024)
            } else {
                date_between(&mut dep_rng, 1950, 2000)
            };
            let cpf = match (child && born.0 >= 2016, dep_rng.below(100)) {
                (true, 0..=59) | (false, 0..=7) => None,
                (_, 0..=79) => Some(Document::Cpf.masked(&dependent.cpf)),
                _ => Some(text_of(&dependent.cpf)),
            };
            let name = if child {
                format!("{} {surname}", dependent.first_name)
            } else {
                dependent.full_name.clone()
            };
            dependentes.push(vec![
                Value::Int(dependentes.len() as i64 + 1),
                Value::Int(id),
                text(name),
                maybe_text(cpf),
                text(date_iso(born)),
                text(kinship),
            ])?;
        }
        employees.push((id, salary_cents, dependents));
    }

    let mut holerites = TableBuilder::new("holerites")
        .primary_key("id", WireType::Integer)
        .foreign_key("funcionario_id", WireType::Integer, "funcionarios", "id")
        .column("competencia", WireType::String)
        .column("salario_bruto", WireType::Float)
        .column("horas_extras", WireType::Float)
        .column("inss", WireType::Float)
        .column("irrf", WireType::Float)
        .column("liquido", WireType::Float);
    let mut pay_rng = root.fork("holerites");
    for competencia in &p.competencias {
        for (id, salary, dependents) in &employees {
            let overtime = if pay_rng.chance(250) {
                pay_rng.between(5_000, salary / 4)
            } else {
                0
            };
            let gross = salary + overtime;
            let inss = inss_cents(gross);
            let irrf = irrf_cents(gross - inss, *dependents);
            holerites.push(vec![
                Value::Int(holerites.len() as i64 + 1),
                Value::Int(*id),
                text(competencia.clone()),
                Value::Float(gross as f64 / 100.0),
                Value::Float(overtime as f64 / 100.0),
                Value::Float(inss as f64 / 100.0),
                Value::Float(irrf as f64 / 100.0),
                Value::Float((gross - inss - irrf) as f64 / 100.0),
            ])?;
        }
    }

    Ok(DatasetDto {
        tables: vec![
            cargos.build(),
            funcionarios.build(),
            dependentes.build(),
            holerites.build(),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inss_and_irrf_follow_their_brackets() {
        assert_eq!(inss_cents(141_200), 10_590);
        assert_eq!(irrf_cents(200_000, 0), 0);
        assert!(irrf_cents(1_000_000, 0) > irrf_cents(1_000_000, 2));
    }
}
