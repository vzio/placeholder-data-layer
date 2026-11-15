//! A marketplace: sellers registered by CNPJ or, as individuals and sole
//! traders, by CPF in the same document column; buyers; orders; shipments
//! with a destination CEP and a carrier tracking code; and buyer reviews
//! whose free text sometimes repeats the buyer's name, phone or email.

use anonymizer::Value;
use serde::Deserialize;
use server::http::dto::{DatasetDto, WireType};

use super::{cpf_in_either_layout, day, pick, weighted};
use crate::generate::company::Company;
use crate::generate::docs::{Document, text_of};
use crate::generate::mess::date_iso;
use crate::generate::person::{Person, cep_masked, phone_masked};
use crate::generate::rng::Rng;
use crate::generate::table::{TableBuilder, maybe_text, text};
use crate::generate::{GenError, params};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MarketplaceParams {
    vendedores: usize,
    /// Per mille of sellers registered by CPF.
    vendedores_pf_por_mil: u64,
    compradores: usize,
    pedidos: usize,
    /// Per mille of delivered orders that receive a review.
    avaliacoes_por_mil: u64,
}

const SOLE_TRADER_TRADES: &[&str] = &[
    "Doces",
    "Artesanato",
    "Bijuterias",
    "Moda Praia",
    "Cerâmicas",
    "Velas Aromáticas",
    "Papelaria Criativa",
    "Pet Shop",
];

const ORDER_STATUS: &[(&str, u64)] = &[
    ("entregue", 640),
    ("em_transito", 120),
    ("aguardando_envio", 80),
    ("cancelado", 90),
    ("devolvido", 30),
    ("aguardando_pagamento", 40),
];

const PAYMENT: &[(&str, u64)] = &[
    ("pix", 42),
    ("cartao_credito", 44),
    ("boleto", 9),
    ("saldo_carteira", 5),
];

const CARRIERS: &[(&str, u64)] = &[
    ("Correios", 62),
    ("Jadlog", 18),
    ("Loggi", 12),
    ("Total Express", 8),
];

const CORREIOS_SERVICE_PREFIXES: &[&str] = &["QB", "OJ", "NX", "PM", "LB", "AA", "OD"];

const PLAIN_REVIEWS: &[&str] = &[
    "Produto chegou antes do prazo, recomendo.",
    "Embalagem amassada mas o produto estava ok.",
    "Exatamente como na foto. Vendedor atencioso.",
    "Demorou bastante para ser postado.",
    "Qualidade abaixo do esperado pelo preço.",
    "Comprei de novo, segunda vez com esse vendedor.",
    "Tamanho veio errado, pedi troca.",
    "Ótimo custo-benefício!",
    "Veio faltando uma peça, aguardando retorno.",
    "Entrega rápida e bem embalado.",
];

const STARS: &[(i64, u64)] = &[(5, 560), (4, 210), (3, 90), (2, 50), (1, 90)];

/// A Correios tracking code (`QB123456789BR`) or another carrier's own
/// layout: Jadlog's fourteen digits, Loggi's prefixed code, Total
/// Express's alphanumeric reference.
fn tracking_code(rng: &mut Rng, carrier: &str) -> String {
    match carrier {
        "Correios" => format!(
            "{}{:09}BR",
            pick(rng, CORREIOS_SERVICE_PREFIXES),
            rng.below(1_000_000_000)
        ),
        "Jadlog" => format!("{:014}", rng.below(100_000_000_000_000)),
        "Loggi" => format!("LGI-{:010}", rng.below(10_000_000_000)),
        _ => format!("TX{:08}SP", rng.below(100_000_000)),
    }
}

/// A buyer's review: usually plain, sometimes giving away the buyer's
/// first name, phone or email.
fn review_text(rng: &mut Rng, buyer: &Person) -> Option<String> {
    let phone = phone_masked(&buyer.phone);
    match rng.below(100) {
        0..=24 => None,
        25..=81 => Some(pick(rng, PLAIN_REVIEWS).to_owned()),
        82..=87 => Some(format!(
            "Vendedor pediu para eu chamar no whats {phone} para combinar a troca."
        )),
        88..=92 => Some(format!(
            "Aqui é {}, comprei de presente e minha mãe adorou.",
            buyer.full_name
        )),
        93..=96 => Some(format!(
            "Não recebi a nota fiscal, mandem para {} por favor.",
            buyer.email
        )),
        _ => Some(format!(
            "Entregaram no vizinho. Meu telefone é {phone}, ninguém ligou."
        )),
    }
}

struct Seller {
    pessoa_fisica: bool,
    documento: String,
    razao_social: String,
    nome_fantasia: String,
    email: String,
    telefone: String,
    cidade: &'static str,
    uf: &'static str,
}

fn seller(rng: &mut Rng, pf_per_mille: u64) -> Seller {
    if rng.chance(pf_per_mille) {
        let owner = Person::generate(rng);
        return Seller {
            pessoa_fisica: true,
            documento: cpf_in_either_layout(rng, &owner.cpf, 800),
            razao_social: format!("{} {}", owner.full_name, text_of(&owner.cpf)),
            nome_fantasia: format!("{} {}", owner.first_name, pick(rng, SOLE_TRADER_TRADES)),
            email: owner.email,
            telefone: phone_masked(&owner.phone),
            cidade: owner.city.name,
            uf: owner.city.uf,
        };
    }
    let company = Company::generate(rng);
    let documento = if rng.chance(800) {
        Document::Cnpj.masked(&company.cnpj)
    } else {
        text_of(&company.cnpj)
    };
    Seller {
        pessoa_fisica: false,
        documento,
        razao_social: company.razao_social,
        nome_fantasia: company.nome_fantasia,
        email: company.email,
        telefone: phone_masked(&company.phone),
        cidade: company.city.name,
        uf: company.city.uf,
    }
}

pub fn generate(seed: u64, raw: &serde_json::Value) -> Result<DatasetDto, GenError> {
    let p: MarketplaceParams = params(raw)?;
    if p.vendedores == 0 || p.compradores == 0 {
        return Err(GenError(
            "a marketplace needs sellers and buyers".to_owned(),
        ));
    }
    let root = Rng::new(seed);

    let mut vendedores = TableBuilder::new("vendedores")
        .primary_key("id", WireType::Integer)
        .column("tipo_pessoa", WireType::String)
        .column("documento", WireType::String)
        .column("razao_social", WireType::String)
        .column("nome_fantasia", WireType::String)
        .column("email", WireType::String)
        .column("telefone", WireType::String)
        .column("cidade", WireType::String)
        .column("uf", WireType::String)
        .column("data_cadastro", WireType::String);
    let mut rng = root.fork("vendedores");
    for index in 0..p.vendedores {
        let s = seller(&mut rng, p.vendedores_pf_por_mil);
        vendedores.push(vec![
            Value::Int(index as i64 + 1),
            text(if s.pessoa_fisica { "PF" } else { "PJ" }),
            text(s.documento),
            text(s.razao_social),
            text(s.nome_fantasia),
            text(s.email),
            text(s.telefone),
            text(s.cidade),
            text(s.uf),
            text(date_iso(day(&mut rng, 2018, 2024))),
        ])?;
    }

    let mut compradores = TableBuilder::new("compradores")
        .primary_key("id", WireType::Integer)
        .column("nome", WireType::String)
        .column("cpf", WireType::String)
        .column("email", WireType::String)
        .column("telefone", WireType::String)
        .column("data_nascimento", WireType::String);
    let mut rng = root.fork("compradores");
    let mut buyers = Vec::with_capacity(p.compradores);
    for index in 0..p.compradores {
        let person = Person::generate(&mut rng);
        let cpf = (!rng.chance(40)).then(|| cpf_in_either_layout(&mut rng, &person.cpf, 700));
        let telefone = (!rng.chance(80)).then(|| phone_masked(&person.phone));
        compradores.push(vec![
            Value::Int(index as i64 + 1),
            text(person.full_name.clone()),
            maybe_text(cpf),
            text(person.email.clone()),
            maybe_text(telefone),
            text(date_iso(person.birth)),
        ])?;
        buyers.push(person);
    }

    let mut pedidos = TableBuilder::new("pedidos")
        .primary_key("id", WireType::Integer)
        .foreign_key("comprador_id", WireType::Integer, "compradores", "id")
        .foreign_key("vendedor_id", WireType::Integer, "vendedores", "id")
        .column("data_pedido", WireType::String)
        .column("status", WireType::String)
        .column("forma_pagamento", WireType::String)
        .column("valor_total", WireType::Float);
    let mut envios = TableBuilder::new("envios")
        .primary_key("id", WireType::Integer)
        .foreign_key("pedido_id", WireType::Integer, "pedidos", "id")
        .column("cep_destino", WireType::String)
        .column("cidade_destino", WireType::String)
        .column("uf_destino", WireType::String)
        .column("transportadora", WireType::String)
        .column("codigo_rastreio", WireType::String)
        .column("prazo_dias", WireType::Integer);
    let mut avaliacoes = TableBuilder::new("avaliacoes")
        .primary_key("id", WireType::Integer)
        .foreign_key("pedido_id", WireType::Integer, "pedidos", "id")
        .column("nota", WireType::Integer)
        .column("comentario", WireType::String)
        .column("data_avaliacao", WireType::String);

    let mut order_rng = root.fork("pedidos");
    let mut shipment_rng = root.fork("envios");
    let mut review_rng = root.fork("avaliacoes");
    let sellers = p.vendedores as u64;
    for row in 0..p.pedidos {
        let order_id = 500_001 + row as i64;
        let buyer_index = order_rng.index(p.compradores);
        // A cubed uniform draw concentrates sales on the first, largest sellers.
        let u = order_rng.below(sellers);
        let seller_id = (u * u / sellers * u / sellers) as i64 + 1;
        let date = day(&mut order_rng, 2023, 2024);
        let status = weighted(&mut order_rng, ORDER_STATUS);
        let cents = order_rng.between(1_990, 89_990) + order_rng.below(3) * 50_000;
        pedidos.push(vec![
            Value::Int(order_id),
            Value::Int(buyer_index as i64 + 1),
            Value::Int(seller_id),
            text(date_iso(date)),
            text(status),
            text(weighted(&mut order_rng, PAYMENT)),
            Value::Float(cents as f64 / 100.0),
        ])?;

        if matches!(status, "cancelado" | "aguardando_pagamento") {
            continue;
        }
        let buyer = &buyers[buyer_index % buyers.len()];
        let carrier = weighted(&mut shipment_rng, CARRIERS);
        let cep = if shipment_rng.chance(750) {
            cep_masked(&buyer.cep)
        } else {
            buyer.cep.clone()
        };
        envios.push(vec![
            Value::Int(envios.len() as i64 + 1),
            Value::Int(order_id),
            text(cep),
            text(buyer.city.name),
            text(buyer.city.uf),
            text(carrier),
            text(tracking_code(&mut shipment_rng, carrier)),
            Value::Int(shipment_rng.between(2, 15) as i64),
        ])?;

        if status == "entregue" && review_rng.chance(p.avaliacoes_por_mil) {
            let stars = review_rng.weighted(STARS).copied().unwrap_or(5);
            let (year, month, _) = date;
            let reviewed = (year, month, review_rng.between(1, 28) as u32);
            avaliacoes.push(vec![
                Value::Int(avaliacoes.len() as i64 + 1),
                Value::Int(order_id),
                Value::Int(stars),
                maybe_text(review_text(&mut review_rng, buyer)),
                text(date_iso(reviewed)),
            ])?;
        }
    }
    Ok(DatasetDto {
        tables: vec![
            vendedores.build(),
            compradores.build(),
            pedidos.build(),
            envios.build(),
            avaliacoes.build(),
        ],
    })
}
