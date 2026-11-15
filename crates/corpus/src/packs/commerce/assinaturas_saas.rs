//! A SaaS billing database: customer accounts, their users with a login
//! handle, subscriptions and invoices. Every entity has a public UUID, and
//! child tables repeat their parents' public ids next to the integer
//! foreign keys, so one UUID names the same account across four tables.

use std::collections::BTreeSet;

use anonymizer::Value;
use serde::Deserialize;
use server::http::dto::{DatasetDto, WireType};

use super::{day, timestamp, uuid, weighted};
use crate::generate::company::Company;
use crate::generate::docs::Document;
use crate::generate::mess::date_iso;
use crate::generate::person::{Person, ascii_fold};
use crate::generate::rng::Rng;
use crate::generate::table::{TableBuilder, maybe_text, text};
use crate::generate::{GenError, params};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaasParams {
    contas: usize,
    usuarios_max_por_conta: u64,
    faturas_max_por_assinatura: u64,
}

/// Plan name and monthly price in cents.
const PLANS: &[((&str, u64), u64)] = &[
    (("starter", 4_900), 50),
    (("pro", 14_900), 30),
    (("business", 39_900), 15),
    (("enterprise", 129_900), 5),
];

const SUBSCRIPTION_STATUS: &[(&str, u64)] = &[
    ("ativa", 70),
    ("trial", 8),
    ("inadimplente", 7),
    ("cancelada", 15),
];

const INVOICE_STATUS: &[(&str, u64)] = &[
    ("paga", 84),
    ("aberta", 7),
    ("vencida", 6),
    ("estornada", 3),
];

const ROLES: &[(&str, u64)] = &[("admin", 20), ("membro", 70), ("financeiro", 10)];

/// A login handle built from the person's name the way people choose
/// them, made unique with a numeric suffix.
fn login_for(rng: &mut Rng, person: &Person, taken: &mut BTreeSet<String>) -> String {
    let first = ascii_fold(&person.first_name).to_lowercase();
    let last = person
        .full_name
        .rsplit(' ')
        .next()
        .map(|s| ascii_fold(s).to_lowercase())
        .unwrap_or_default();
    let initial = first.get(..1).unwrap_or("x");
    let mut login = match rng.below(5) {
        0 => format!("{first}.{last}"),
        1 => format!("{initial}{last}"),
        2 => format!("{first}_{}", last.get(..1).unwrap_or("x")),
        3 => format!("{last}.{first}"),
        _ => format!("{first}{}", person.birth.0 % 100),
    };
    while taken.contains(&login) {
        login = format!("{login}{}", rng.between(1, 9));
    }
    taken.insert(login.clone());
    login
}

pub fn generate(seed: u64, raw: &serde_json::Value) -> Result<DatasetDto, GenError> {
    let p: SaasParams = params(raw)?;
    let root = Rng::new(seed);

    let mut contas = TableBuilder::new("contas")
        .primary_key("id", WireType::Integer)
        .column("public_id", WireType::String)
        .column("razao_social", WireType::String)
        .column("cnpj", WireType::String)
        .column("email_financeiro", WireType::String)
        .column("criada_em", WireType::String);
    let mut usuarios = TableBuilder::new("usuarios")
        .primary_key("id", WireType::Integer)
        .column("public_id", WireType::String)
        .foreign_key("conta_id", WireType::Integer, "contas", "id")
        .column("conta_public_id", WireType::String)
        .column("nome", WireType::String)
        .column("email", WireType::String)
        .column("login", WireType::String)
        .column("papel", WireType::String)
        .column("ultimo_acesso", WireType::String);
    let mut assinaturas = TableBuilder::new("assinaturas")
        .primary_key("id", WireType::Integer)
        .column("public_id", WireType::String)
        .foreign_key("conta_id", WireType::Integer, "contas", "id")
        .column("conta_public_id", WireType::String)
        .column("plano", WireType::String)
        .column("ciclo", WireType::String)
        .column("status", WireType::String)
        .column("inicio", WireType::String)
        .column("valor_ciclo", WireType::Float);
    let mut faturas = TableBuilder::new("faturas")
        .primary_key("id", WireType::Integer)
        .column("public_id", WireType::String)
        .foreign_key("assinatura_id", WireType::Integer, "assinaturas", "id")
        .column("assinatura_public_id", WireType::String)
        .column("conta_public_id", WireType::String)
        .column("emitida_em", WireType::String)
        .column("valor", WireType::Float)
        .column("status", WireType::String)
        .column("numero_nfse", WireType::Integer);

    let mut account_rng = root.fork("contas");
    let mut user_rng = root.fork("usuarios");
    let mut subscription_rng = root.fork("assinaturas");
    let mut invoice_rng = root.fork("faturas");
    let mut logins = BTreeSet::new();
    let mut nfse = 2_024_000_i64;
    for index in 0..p.contas {
        let account_id = index as i64 + 1;
        let account_uuid = uuid(&mut account_rng);
        let company = Company::generate(&mut account_rng);
        let created = day(&mut account_rng, 2019, 2023);
        let domain = company
            .email
            .rsplit_once('@')
            .map(|(_, d)| d.to_owned())
            .unwrap_or_default();
        contas.push(vec![
            Value::Int(account_id),
            text(account_uuid.clone()),
            text(company.razao_social),
            text(Document::Cnpj.masked(&company.cnpj)),
            text(format!("financeiro@{domain}")),
            text(date_iso(created)),
        ])?;

        for _ in 0..user_rng.between(1, p.usuarios_max_por_conta) {
            let person = Person::generate(&mut user_rng);
            let login = login_for(&mut user_rng, &person, &mut logins);
            let email = if user_rng.chance(850) {
                let local = person.email.split('@').next().unwrap_or("usuario");
                format!("{local}@{domain}")
            } else {
                person.email.clone()
            };
            let seen = user_rng.chance(900).then(|| {
                let date = day(&mut user_rng, 2024, 2024);
                timestamp(&mut user_rng, date)
            });
            usuarios.push(vec![
                Value::Int(usuarios.len() as i64 + 1),
                text(uuid(&mut user_rng)),
                Value::Int(account_id),
                text(account_uuid.clone()),
                text(person.full_name),
                text(email),
                text(login),
                text(weighted(&mut user_rng, ROLES)),
                maybe_text(seen),
            ])?;
        }

        let subscriptions = if subscription_rng.chance(250) { 2 } else { 1 };
        for order in 0..subscriptions {
            let subscription_id = assinaturas.len() as i64 + 1;
            let subscription_uuid = uuid(&mut subscription_rng);
            let &(plan, monthly) = subscription_rng
                .weighted(PLANS)
                .unwrap_or(&("starter", 4_900));
            let annual = subscription_rng.chance(300);
            let cycle_cents = if annual { monthly * 10 } else { monthly };
            let status = if subscriptions == 2 && order == 0 {
                "cancelada"
            } else {
                weighted(&mut subscription_rng, SUBSCRIPTION_STATUS)
            };
            let start = day(&mut subscription_rng, u64::from(created.0), 2024);
            assinaturas.push(vec![
                Value::Int(subscription_id),
                text(subscription_uuid.clone()),
                Value::Int(account_id),
                text(account_uuid.clone()),
                text(plan),
                text(if annual { "anual" } else { "mensal" }),
                text(status),
                text(date_iso(start)),
                Value::Float(cycle_cents as f64 / 100.0),
            ])?;

            let invoices = match status {
                "trial" => 0,
                _ if annual => invoice_rng.between(1, 3),
                _ => invoice_rng.between(1, p.faturas_max_por_assinatura),
            };
            for month in 0..invoices {
                let (year, start_month, issue_day) = start;
                let months = u64::from(start_month - 1) + month;
                let issued = (
                    year + (months / 12) as u32,
                    (months % 12) as u32 + 1,
                    issue_day,
                );
                let invoice_status = weighted(&mut invoice_rng, INVOICE_STATUS);
                let number =
                    (invoice_status != "estornada" || invoice_rng.chance(500)).then(|| {
                        nfse += 1;
                        nfse
                    });
                faturas.push(vec![
                    Value::Int(faturas.len() as i64 + 1),
                    text(uuid(&mut invoice_rng)),
                    Value::Int(subscription_id),
                    text(subscription_uuid.clone()),
                    text(account_uuid.clone()),
                    text(date_iso(issued)),
                    Value::Float(cycle_cents as f64 / 100.0),
                    text(invoice_status),
                    number.map_or(Value::Null, Value::Int),
                ])?;
            }
        }
    }

    Ok(DatasetDto {
        tables: vec![
            contas.build(),
            usuarios.build(),
            assinaturas.build(),
            faturas.build(),
        ],
    })
}
