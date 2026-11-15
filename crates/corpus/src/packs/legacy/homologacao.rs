//! A staging database cloned from production and then used for QA: a
//! seeded administrator at id 1, pure test accounts, real customers with a
//! test value typed into one or two fields, test accounts the QA team named
//! in its own house style, and real customers whose names only look like
//! test words.

use anonymizer::Value;
use serde::Deserialize;
use server::http::dto::{DatasetDto, WireType};

use crate::generate::docs::Document;
use crate::generate::mess::date_iso;
use crate::generate::person::{Person, ascii_fold, phone_masked};
use crate::generate::rng::Rng;
use crate::generate::table::{TableBuilder, maybe_text, text};
use crate::generate::{GenError, params};

const QA_NAMES: &[&str] = &[
    "Fulano de Tal",
    "Beltrano da Silva",
    "Ciclano Souza",
    "Sicrano de Oliveira",
    "Teste QA",
    "Usuario Teste",
    "Teste Automatizado",
];

const QA_CPFS: &[&str] = &[
    "999.999.999-99",
    "111.111.111-11",
    "000.000.000-00",
    "123.456.789-09",
    "111.444.777-35",
];

const QA_PHONES: &[&str] = &[
    "(00) 00000-0000",
    "(11) 99999-9999",
    "(11) 11111-1111",
    "(21) 00000-0000",
];

/// Surnames that contain a test or role word without being one.
const LOOKALIKE_SURNAMES: &[&str] = &["Testa", "Testoni", "Admiral", "Demo", "Fulani", "Rootes"];

const ORDER_STATUS: &[(&str, u64)] = &[
    ("entregue", 62),
    ("enviado", 15),
    ("pago", 10),
    ("cancelado", 9),
    ("estornado", 4),
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HomologacaoParams {
    usuarios: usize,
    qa_puros: usize,
    mistos: usize,
    qa_estilo_proprio: usize,
    parecidos: usize,
    pedidos: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Account {
    Production,
    PureQa,
    Mixed,
    HouseStyleQa,
    Lookalike,
}

struct Fields {
    nome: String,
    email: String,
    cpf: String,
    telefone: Option<String>,
    perfil: &'static str,
}

fn production(person: &Person) -> Fields {
    Fields {
        nome: person.full_name.clone(),
        email: person.email.clone(),
        cpf: Document::Cpf.masked(&person.cpf),
        telefone: Some(phone_masked(&person.phone)),
        perfil: "cliente",
    }
}

fn pick(rng: &mut Rng, items: &[&str]) -> String {
    rng.pick(items).copied().unwrap_or_default().to_owned()
}

fn pure_qa(rng: &mut Rng, serial: usize) -> Fields {
    let email = match rng.below(4) {
        0 => format!("qa+{serial}@mailinator.com"),
        1 => format!("teste{serial}@teste.com"),
        2 => format!("fulano{serial}@example.com"),
        _ => format!("teste+{serial}@gmail.com"),
    };
    Fields {
        nome: pick(rng, QA_NAMES),
        email,
        cpf: pick(rng, QA_CPFS),
        telefone: Some(pick(rng, QA_PHONES)),
        perfil: if rng.chance(300) { "qa" } else { "cliente" },
    }
}

/// A production row with one or two fields overwritten by a tester.
fn mixed(rng: &mut Rng, person: &Person) -> Fields {
    let mut fields = production(person);
    let local = person
        .email
        .split_once('@')
        .map_or("cliente", |(local, _)| local);
    match rng.below(5) {
        0 => fields.email = format!("{local}@mailinator.com"),
        1 => fields.cpf = "999.999.999-99".to_owned(),
        2 => fields.telefone = Some("(11) 99999-9999".to_owned()),
        3 => fields.nome = format!("Teste {}", person.full_name),
        _ => {
            fields.email = format!("{local}@yopmail.com");
            fields.cpf = "111.111.111-11".to_owned();
        }
    }
    fields
}

/// A test account in the QA team's own convention, which no generic
/// placeholder list knows: plausible values, a team prefix.
fn house_style_qa(rng: &mut Rng, serial: usize, company: &str) -> Fields {
    let cpf = Document::Cpf.generate(rng);
    Fields {
        nome: format!("QA Automação {serial:02}"),
        email: format!("qa.automacao{serial:02}@{company}"),
        cpf: Document::Cpf.masked(&cpf),
        telefone: Some(format!("(11) 91234-56{:02}", serial % 100)),
        perfil: "cliente",
    }
}

fn lookalike(rng: &mut Rng, person: &Person) -> Fields {
    let surname = pick(rng, LOOKALIKE_SURNAMES);
    let first = ascii_fold(&person.first_name).to_lowercase();
    let mut fields = production(person);
    fields.nome = format!("{} {surname}", person.first_name);
    fields.email = format!(
        "{first}.{}@{}",
        ascii_fold(&surname).to_lowercase(),
        person.email.split_once('@').map_or("gmail.com", |(_, d)| d)
    );
    fields
}

/// Which account each row after the administrator is, drawn without
/// replacement so every kind has exactly its count.
fn account_kinds(rng: &mut Rng, p: &HomologacaoParams) -> Result<Vec<Account>, GenError> {
    let special = p.qa_puros + p.mistos + p.qa_estilo_proprio + p.parecidos;
    let rows = p.usuarios.saturating_sub(1);
    if special > rows {
        return Err(GenError("more special accounts than users".to_owned()));
    }
    let mut kinds = vec![Account::Production; rows];
    let wanted = [
        (Account::PureQa, p.qa_puros),
        (Account::Mixed, p.mistos),
        (Account::HouseStyleQa, p.qa_estilo_proprio),
        (Account::Lookalike, p.parecidos),
    ];
    for (kind, count) in wanted {
        let mut placed = 0;
        while placed < count {
            let row = rng.index(rows);
            if kinds[row] == Account::Production {
                kinds[row] = kind;
                placed += 1;
            }
        }
    }
    Ok(kinds)
}

pub fn ambiente_homologacao_misturado(
    seed: u64,
    raw: &serde_json::Value,
) -> Result<DatasetDto, GenError> {
    let p: HomologacaoParams = params(raw)?;
    let root = Rng::new(seed);
    let kinds = account_kinds(&mut root.fork("tipos"), &p)?;
    let company = "lojavirtualexemplar.com.br";

    let mut usuarios = TableBuilder::new("usuarios")
        .primary_key("id", WireType::Integer)
        .column("nome", WireType::String)
        .column("email", WireType::String)
        .column("cpf", WireType::String)
        .column("telefone", WireType::String)
        .column("perfil", WireType::String)
        .column("criado_em", WireType::String);
    usuarios.push(vec![
        Value::Int(1),
        text("Administrador do Sistema"),
        text(format!("admin@{company}")),
        text("000.000.000-00"),
        Value::Null,
        text("admin"),
        text("2015-01-01"),
    ])?;
    let mut rng = root.fork("usuarios");
    let mut qa_serial = 0;
    let mut house_serial = 0;
    for (index, kind) in kinds.iter().enumerate() {
        let person = Person::generate(&mut rng);
        let fields = match kind {
            Account::Production => production(&person),
            Account::PureQa => {
                qa_serial += 1;
                pure_qa(&mut rng, qa_serial)
            }
            Account::Mixed => mixed(&mut rng, &person),
            Account::HouseStyleQa => {
                house_serial += 1;
                house_style_qa(&mut rng, house_serial, company)
            }
            Account::Lookalike => lookalike(&mut rng, &person),
        };
        let created = (
            2015 + (index * 9 / kinds.len().max(1)) as u32,
            rng.between(1, 12) as u32,
            rng.between(1, 28) as u32,
        );
        usuarios.push(vec![
            Value::Int(index as i64 + 2),
            text(fields.nome),
            text(fields.email),
            text(fields.cpf),
            maybe_text(fields.telefone),
            text(fields.perfil),
            text(date_iso(created)),
        ])?;
    }

    let mut rng = root.fork("pedidos");
    let users = p.usuarios as u64;
    let mut pedidos = TableBuilder::new("pedidos")
        .primary_key("id", WireType::Integer)
        .foreign_key("usuario_id", WireType::Integer, "usuarios", "id")
        .foreign_key("criado_por", WireType::Integer, "usuarios", "id")
        .column("valor", WireType::Float)
        .column("status", WireType::String);
    for row in 0..p.pedidos {
        let creator = if rng.chance(850) {
            1
        } else {
            rng.between(2, users) as i64
        };
        pedidos.push(vec![
            Value::Int(row as i64 + 1),
            Value::Int(rng.between(2, users) as i64),
            Value::Int(creator),
            Value::Float(rng.between(990, 89_990) as f64 / 100.0),
            text(rng.weighted(ORDER_STATUS).copied().unwrap_or("entregue")),
        ])?;
    }

    Ok(DatasetDto {
        tables: vec![usuarios.build(), pedidos.build()],
    })
}
