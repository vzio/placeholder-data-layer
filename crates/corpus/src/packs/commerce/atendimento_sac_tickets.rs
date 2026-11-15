//! A customer-service ticket system: customers, tickets whose description
//! is long operator prose full of CPFs, phones, emails and names (the
//! customer's and third parties'), and the messages exchanged on each
//! ticket. Every ticket has an eleven-digit protocol number that fails the
//! CPF check-digit rule: a number shaped like a CPF that is not one.

use anonymizer::Value;
use serde::Deserialize;
use server::http::dto::{DatasetDto, WireType};

use super::{cpf_in_either_layout, day, eleven_digits_failing_cpf, pick, timestamp, weighted};
use crate::generate::docs::Document;
use crate::generate::person::{Person, phone_masked};
use crate::generate::rng::Rng;
use crate::generate::table::{TableBuilder, text};
use crate::generate::{GenError, params};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SacParams {
    clientes: usize,
    atendentes: usize,
    tickets: usize,
    mensagens_max_por_ticket: u64,
}

const CHANNELS: &[(&str, u64)] = &[
    ("telefone", 38),
    ("chat", 30),
    ("email", 17),
    ("whatsapp", 12),
    ("reclame_aqui", 3),
];

const SUBJECTS: &[(&str, u64)] = &[
    ("atraso_entrega", 30),
    ("cobranca_indevida", 18),
    ("troca_devolucao", 20),
    ("produto_com_defeito", 14),
    ("cancelamento", 10),
    ("alteracao_cadastral", 8),
];

const PRIORITIES: &[(&str, u64)] = &[("baixa", 30), ("media", 50), ("alta", 15), ("urgente", 5)];

const STATUS: &[(&str, u64)] = &[
    ("resolvido", 62),
    ("em_andamento", 18),
    ("aguardando_cliente", 12),
    ("reaberto", 8),
];

const PROBLEMS: &[&str] = &[
    "o pedido não foi entregue no prazo",
    "foi cobrado duas vezes no cartão",
    "o produto chegou com defeito",
    "quer cancelar a compra e receber o estorno",
    "não consegue alterar o endereço de entrega",
    "recebeu um produto diferente do comprado",
];

const RESOLUTIONS: &[&str] = &[
    "Encaminhado para o financeiro.",
    "Aberta solicitação de coleta junto à transportadora.",
    "Orientado a aguardar 5 dias úteis.",
    "Estorno solicitado, prazo de duas faturas.",
    "Cliente ficou de enviar fotos do produto.",
    "Caso escalado para a supervisão.",
];

const RELATIONS: &[&str] = &["esposa", "marido", "filha", "filho", "mãe", "irmão"];

const CUSTOMER_LINES: &[&str] = &[
    "Bom dia, até agora nada foi resolvido.",
    "Já liguei três vezes e ninguém me retorna.",
    "Ok, fico no aguardo.",
    "Obrigado pelo retorno.",
    "Isso é um absurdo, vou reclamar no Procon.",
];

const AGENT_LINES: &[&str] = &[
    "Estamos verificando com o setor responsável.",
    "Sua solicitação foi encaminhada.",
    "Pedimos desculpas pelo transtorno.",
    "O prazo para retorno é de até 48 horas.",
];

/// A ticket description: the complaint, then whatever identifying details
/// the operator typed in, then the resolution.
fn description(rng: &mut Rng, customer: &Person, channel: &str) -> String {
    let mut parts = vec![format!(
        "Cliente {} entrou em contato via {channel} informando que {}.",
        customer.full_name,
        pick(rng, PROBLEMS)
    )];
    if rng.chance(650) {
        parts.push(format!(
            "Confirmou CPF {} para validação.",
            cpf_in_either_layout(rng, &customer.cpf, 700)
        ));
    }
    if rng.chance(500) {
        parts.push(format!(
            "Pediu retorno no telefone {}.",
            phone_masked(&customer.phone)
        ));
    }
    if rng.chance(350) {
        parts.push(format!("Enviar comprovante para {}.", customer.email));
    }
    if rng.chance(200) {
        let other = Person::generate(rng);
        parts.push(format!(
            "Compra feita no nome da {} {}, CPF {}, contato {}.",
            pick(rng, RELATIONS),
            other.full_name,
            Document::Cpf.masked(&other.cpf),
            phone_masked(&other.phone)
        ));
    }
    parts.push(pick(rng, RESOLUTIONS).to_owned());
    parts.join(" ")
}

fn message(rng: &mut Rng, from_customer: bool, customer: &Person, protocol: &str) -> String {
    if from_customer {
        match rng.below(100) {
            0..=54 => pick(rng, CUSTOMER_LINES).to_owned(),
            55..=69 => format!("Meu protocolo é {protocol}, alguém pode ver?"),
            70..=84 => format!(
                "Segue meu CPF: {}",
                cpf_in_either_layout(rng, &customer.cpf, 500)
            ),
            _ => format!("Podem me ligar no {}", phone_masked(&customer.phone)),
        }
    } else {
        match rng.below(100) {
            0..=59 => pick(rng, AGENT_LINES).to_owned(),
            _ => format!(
                "Olá, {}! Seu protocolo de atendimento é {protocol}.",
                customer.first_name
            ),
        }
    }
}

pub fn generate(seed: u64, raw: &serde_json::Value) -> Result<DatasetDto, GenError> {
    let p: SacParams = params(raw)?;
    if p.clientes == 0 || p.atendentes == 0 {
        return Err(GenError("tickets need customers and agents".to_owned()));
    }
    let root = Rng::new(seed);

    let mut clientes = TableBuilder::new("clientes")
        .primary_key("id", WireType::Integer)
        .column("nome", WireType::String)
        .column("cpf", WireType::String)
        .column("email", WireType::String)
        .column("telefone", WireType::String);
    let mut rng = root.fork("clientes");
    let mut customers = Vec::with_capacity(p.clientes);
    for index in 0..p.clientes {
        let person = Person::generate(&mut rng);
        clientes.push(vec![
            Value::Int(index as i64 + 1),
            text(person.full_name.clone()),
            text(Document::Cpf.masked(&person.cpf)),
            text(person.email.clone()),
            text(phone_masked(&person.phone)),
        ])?;
        customers.push(person);
    }

    let mut agent_rng = root.fork("atendentes");
    let agents: Vec<String> = (0..p.atendentes)
        .map(|_| Person::generate(&mut agent_rng).full_name)
        .collect();

    let mut tickets = TableBuilder::new("tickets")
        .primary_key("id", WireType::Integer)
        .column("protocolo", WireType::String)
        .foreign_key("cliente_id", WireType::Integer, "clientes", "id")
        .column("nome_cliente", WireType::String)
        .column("email_cliente", WireType::String)
        .column("canal", WireType::String)
        .column("assunto", WireType::String)
        .column("prioridade", WireType::String)
        .column("status", WireType::String)
        .column("aberto_em", WireType::String)
        .column("atendente", WireType::String)
        .column("descricao", WireType::String);
    let mut mensagens = TableBuilder::new("mensagens")
        .primary_key("id", WireType::Integer)
        .foreign_key("ticket_id", WireType::Integer, "tickets", "id")
        .column("autor", WireType::String)
        .column("texto", WireType::String);

    let mut ticket_rng = root.fork("tickets");
    let mut message_rng = root.fork("mensagens");
    let mut sequence = 0_u64;
    for row in 0..p.tickets {
        let ticket_id = row as i64 + 1;
        let customer_index = ticket_rng.index(customers.len());
        let customer = &customers[customer_index];
        let opened = day(&mut ticket_rng, 2024, 2024);
        let year = opened.0;
        let protocol = eleven_digits_failing_cpf(|| {
            sequence += ticket_rng.between(1, 40);
            format!("{year}{sequence:07}")
        });
        let channel = weighted(&mut ticket_rng, CHANNELS);
        tickets.push(vec![
            Value::Int(ticket_id),
            text(protocol.clone()),
            Value::Int(customer_index as i64 + 1),
            text(customer.full_name.clone()),
            text(customer.email.clone()),
            text(channel),
            text(weighted(&mut ticket_rng, SUBJECTS)),
            text(weighted(&mut ticket_rng, PRIORITIES)),
            text(weighted(&mut ticket_rng, STATUS)),
            text(timestamp(&mut ticket_rng, opened)),
            text(pick_agent(&mut ticket_rng, &agents)),
            text(description(&mut ticket_rng, customer, channel)),
        ])?;

        for turn in 0..message_rng.between(1, p.mensagens_max_por_ticket) {
            let from_customer = turn % 2 == 0;
            mensagens.push(vec![
                Value::Int(mensagens.len() as i64 + 1),
                Value::Int(ticket_id),
                text(if from_customer {
                    "cliente"
                } else {
                    "atendente"
                }),
                text(message(
                    &mut message_rng,
                    from_customer,
                    customer,
                    &protocol,
                )),
            ])?;
        }
    }

    Ok(DatasetDto {
        tables: vec![clientes.build(), tickets.build(), mensagens.build()],
    })
}

fn pick_agent(rng: &mut Rng, agents: &[String]) -> String {
    rng.pick(agents).cloned().unwrap_or_default()
}
