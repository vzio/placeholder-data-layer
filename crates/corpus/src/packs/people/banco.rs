//! A digital bank: individual and business customers, their accounts,
//! the PIX keys registered to each account, and a ledger of transactions
//! whose amounts have a heavy right tail and whose PIX transfers name the
//! destination key and the counterparty.

use anonymizer::Value;
use serde::Deserialize;
use server::http::dto::{DatasetDto, WireType};

use super::ids::{bank_account, bank_branch, uuid_v4};
use super::words::{MERCHANTS, TIPOS_CONTA, TRANSACTION_KINDS};
use super::{date_between, heavy_tailed_cents};
use crate::generate::company::Company;
use crate::generate::docs::{Document, text_of};
use crate::generate::mess::date_iso;
use crate::generate::person::{Person, phone_masked};
use crate::generate::rng::Rng;
use crate::generate::table::{TableBuilder, maybe_text, text};
use crate::generate::{GenError, params};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BancoParams {
    clientes: usize,
    contas_max_por_cliente: u64,
    transacoes: usize,
    orfas: usize,
}

struct Holder {
    nome: String,
    business: bool,
    document: Vec<u8>,
    email: String,
    /// Area code and number, digits only.
    phone: String,
}

impl Holder {
    fn generate(rng: &mut Rng) -> Holder {
        if rng.chance(180) {
            let company = Company::generate(rng);
            return Holder {
                nome: company.razao_social,
                business: true,
                document: company.cnpj,
                email: company.email,
                phone: company.phone,
            };
        }
        let person = Person::generate(rng);
        Holder {
            nome: person.full_name,
            business: false,
            document: person.cpf,
            email: person.email,
            phone: person.phone,
        }
    }

    fn document_masked(&self) -> String {
        if self.business {
            Document::Cnpj.masked(&self.document)
        } else {
            Document::Cpf.masked(&self.document)
        }
    }
}

/// A PIX key as the directory stores it: documents as bare digits, phones
/// in E.164, emails as typed, random keys as lowercase UUIDs.
fn pix_key(rng: &mut Rng, holder: &Holder, kind: &str) -> String {
    match kind {
        "cpf" | "cnpj" => text_of(&holder.document),
        "email" => holder.email.clone(),
        "telefone" => format!("+55{}", holder.phone),
        _ => uuid_v4(rng),
    }
}

/// Short form of a name as bank statements print it: first name and the
/// initial of the last.
fn statement_name(full: &str) -> String {
    let mut words = full.split_whitespace();
    let first = words.next().unwrap_or("Cliente");
    match words.next_back() {
        Some(last) => format!("{first} {}", last.chars().next().unwrap_or('S')),
        None => first.to_owned(),
    }
}

pub fn generate(seed: u64, raw: &serde_json::Value) -> Result<DatasetDto, GenError> {
    let p: BancoParams = params(raw)?;
    let root = Rng::new(seed);

    let mut clientes = TableBuilder::new("clientes")
        .primary_key("id", WireType::Integer)
        .column("tipo_pessoa", WireType::String)
        .column("nome", WireType::String)
        .column("documento", WireType::String)
        .column("email", WireType::String)
        .column("telefone", WireType::String)
        .column("data_cadastro", WireType::String);
    let mut contas = TableBuilder::new("contas")
        .primary_key("id", WireType::Integer)
        .foreign_key("cliente_id", WireType::Integer, "clientes", "id")
        .column("agencia", WireType::String)
        .column("numero", WireType::String)
        .column("tipo", WireType::String)
        .column("saldo", WireType::Float);
    let mut chaves = TableBuilder::new("chaves_pix")
        .primary_key("id", WireType::Integer)
        .foreign_key("conta_id", WireType::Integer, "contas", "id")
        .column("tipo_chave", WireType::String)
        .column("chave", WireType::String)
        .column("criada_em", WireType::String);

    let mut rng = root.fork("clientes");
    let mut account_rng = root.fork("contas");
    let mut key_rng = root.fork("chaves");
    let mut holders = Vec::with_capacity(p.clientes);
    let mut registered: Vec<(i64, String, String)> = Vec::new();
    for index in 0..p.clientes {
        let holder = Holder::generate(&mut rng);
        let id = index as i64 + 1;
        clientes.push(vec![
            Value::Int(id),
            text(if holder.business { "PJ" } else { "PF" }),
            text(holder.nome.clone()),
            text(holder.document_masked()),
            maybe_text((!rng.chance(40)).then(|| holder.email.clone())),
            text(phone_masked(&holder.phone)),
            text(date_iso(date_between(&mut rng, 2018, 2024))),
        ])?;
        let document_kind = if holder.business { "cnpj" } else { "cpf" };
        let mut kinds_left = vec![document_kind, "email", "telefone"];
        for _ in 0..account_rng.between(1, p.contas_max_por_cliente) {
            let account_id = contas.len() as i64 + 1;
            let balance = if account_rng.chance(80) {
                -(account_rng.between(1_000, 500_000) as i64)
            } else {
                heavy_tailed_cents(
                    &mut account_rng,
                    &[
                        (100, 20),
                        (10_000, 45),
                        (100_000, 25),
                        (1_000_000, 8),
                        (10_000_000, 2),
                    ],
                ) as i64
            };
            contas.push(vec![
                Value::Int(account_id),
                Value::Int(id),
                text(bank_branch(&mut account_rng)),
                text(bank_account(&mut account_rng)),
                text(
                    account_rng
                        .weighted(TIPOS_CONTA)
                        .copied()
                        .unwrap_or("corrente"),
                ),
                Value::Float(balance as f64 / 100.0),
            ])?;
            for _ in 0..key_rng.below(4) {
                let kind = if kinds_left.is_empty() || key_rng.chance(250) {
                    "aleatoria"
                } else {
                    kinds_left.remove(key_rng.index(kinds_left.len()))
                };
                let key = pix_key(&mut key_rng, &holder, kind);
                chaves.push(vec![
                    Value::Int(chaves.len() as i64 + 1),
                    Value::Int(account_id),
                    text(kind),
                    text(key.clone()),
                    text(date_iso(date_between(&mut key_rng, 2020, 2024))),
                ])?;
                registered.push((account_id, key, statement_name(&holder.nome)));
            }
        }
        holders.push(holder);
    }

    let mut transacoes = TableBuilder::new("transacoes")
        .primary_key("id", WireType::Integer)
        .foreign_key("conta_id", WireType::Integer, "contas", "id")
        .column("data_hora", WireType::String)
        .column("tipo", WireType::String)
        .column("valor", WireType::Float)
        .column("chave_pix_destino", WireType::String)
        .column("nome_contraparte", WireType::String)
        .column("descricao", WireType::String);
    let mut tx_rng = root.fork("transacoes");
    let accounts = contas.len() as u64;
    let mut orphan_rows = Vec::new();
    while orphan_rows.len() < p.orfas.min(p.transacoes) {
        let row = tx_rng.index(p.transacoes);
        if !orphan_rows.contains(&row) {
            orphan_rows.push(row);
        }
    }
    let mut outsider_rng = root.fork("contrapartes");
    for row in 0..p.transacoes {
        let account = if orphan_rows.contains(&row) {
            accounts as i64 + 1000 + tx_rng.between(1, 500) as i64
        } else {
            tx_rng.between(1, accounts) as i64
        };
        let kind = tx_rng
            .weighted(TRANSACTION_KINDS)
            .copied()
            .unwrap_or("debito");
        let date = date_between(&mut tx_rng, 2024, 2024);
        let when = format!(
            "{}T{:02}:{:02}:{:02}",
            date_iso(date),
            tx_rng.below(24),
            tx_rng.below(60),
            tx_rng.below(60)
        );
        let amount = match kind {
            "tarifa" => tx_rng.between(150, 4_990),
            "saque" => tx_rng.between(2, 100) * 1_000,
            _ => heavy_tailed_cents(
                &mut tx_rng,
                &[
                    (100, 30),
                    (1_000, 35),
                    (10_000, 25),
                    (100_000, 8),
                    (1_000_000, 2),
                ],
            ),
        };
        let (key, counterparty, description) = match kind {
            "pix_enviado" | "pix_recebido" => {
                let (key, name) = if !registered.is_empty() && tx_rng.chance(550) {
                    let (_, key, name) = &registered[tx_rng.index(registered.len())];
                    (key.clone(), name.clone())
                } else {
                    let outsider = Holder::generate(&mut outsider_rng);
                    let kind = ["cpf", "email", "telefone", "aleatoria"][outsider_rng.index(4)];
                    let kind = if outsider.business && kind == "cpf" {
                        "cnpj"
                    } else {
                        kind
                    };
                    (
                        pix_key(&mut outsider_rng, &outsider, kind),
                        statement_name(&outsider.nome),
                    )
                };
                let verb = if kind == "pix_enviado" {
                    "PIX enviado para"
                } else {
                    "PIX recebido de"
                };
                let description = format!("{verb} {name}");
                (
                    (kind == "pix_enviado").then_some(key),
                    Some(name),
                    description,
                )
            }
            "debito" | "boleto" => {
                let merchant = tx_rng.pick(MERCHANTS).copied().unwrap_or("LOJAS HORIZONTE");
                (None, None, format!("Compra débito {merchant}"))
            }
            "ted" => {
                let other = &holders[tx_rng.index(holders.len())];
                let name = statement_name(&other.nome);
                (None, Some(name.clone()), format!("TED {name}"))
            }
            "tarifa" => (None, None, "Tarifa pacote de serviços".to_owned()),
            _ => (None, None, "Saque 24h".to_owned()),
        };
        transacoes.push(vec![
            Value::Int(row as i64 + 1),
            Value::Int(account),
            text(when),
            text(kind),
            Value::Float(amount as f64 / 100.0),
            maybe_text(key),
            maybe_text(counterparty),
            text(description),
        ])?;
    }

    Ok(DatasetDto {
        tables: vec![
            clientes.build(),
            contas.build(),
            chaves.build(),
            transacoes.build(),
        ],
    })
}
