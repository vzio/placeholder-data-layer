//! The seed pack: a retail customer-and-orders database and a B2B supplier
//! registry, each with the mess a real production dump carries.

use anonymizer::Value;
use serde::Deserialize;
use server::http::dto::{DatasetDto, WireType};

use crate::generate::company::Company;
use crate::generate::docs::{Document, text_of, without_leading_zeros};
use crate::generate::mess::{
    IMPOSSIBLE_DATES, PLACEHOLDER_CPF, PLACEHOLDER_EMAIL, PLACEHOLDER_NAME, PLACEHOLDER_PHONE,
    PLAIN_NOTES, SENTINELS, case_mess, date_br, date_iso, domain_typo, mojibake, note_embedding,
    pad, without_at_sign,
};
use crate::generate::person::{Person, cep_masked, phone_masked};
use crate::generate::rng::Rng;
use crate::generate::table::{TableBuilder, maybe_text, text};
use crate::generate::vocab::{JOB_TITLES, STATES};
use crate::generate::{GenError, GeneratorFn, params};

pub fn generator(name: &str) -> Option<GeneratorFn> {
    match name {
        "varejo_clientes_pedidos" => Some(varejo_clientes_pedidos),
        "fornecedores_b2b" => Some(fornecedores_b2b),
        _ => None,
    }
}

const PRODUCTS: &[(&str, u64)] = &[
    ("Camiseta básica algodão", 3990),
    ("Calça jeans slim", 15990),
    ("Tênis de corrida", 29990),
    ("Fone de ouvido bluetooth", 19990),
    ("Garrafa térmica 1L", 8990),
    ("Mochila escolar", 12990),
    ("Panela antiaderente 24cm", 11990),
    ("Jogo de toalhas", 7990),
    ("Carregador USB-C", 4990),
    ("Cafeteira elétrica", 18990),
    ("Livro de receitas", 5990),
    ("Kit maquiagem", 9990),
];

const ORDER_STATUS: &[(&str, u64)] = &[
    ("entregue", 600),
    ("enviado", 150),
    ("pago", 100),
    ("cancelado", 80),
    ("aguardando_pagamento", 50),
    ("devolvido", 20),
];

const PAYMENT: &[(&str, u64)] = &[("pix", 45), ("cartao_credito", 40), ("boleto", 15)];

fn pick_sentinel(rng: &mut Rng) -> String {
    rng.pick(SENTINELS).copied().unwrap_or("N/A").to_owned()
}

/// The CPF column's mix: masked, bare digits, null, wrong check digits,
/// too short, and a sentinel.
fn messy_cpf(rng: &mut Rng, digits: &[u8]) -> Option<String> {
    let format = *rng.weighted(&[(0, 55), (1, 30), (2, 6), (3, 4), (4, 2), (5, 3)])?;
    match format {
        0 => Some(Document::Cpf.masked(digits)),
        1 => Some(text_of(digits)),
        2 => None,
        3 => Some(Document::Cpf.masked(&Document::Cpf.generate_invalid(rng))),
        4 => {
            let mut short = digits[..10].to_vec();
            if Document::Cpf.is_valid(&text_of(&short)) {
                short[9] = (short[9] + 1) % 10;
            }
            Some(text_of(&short))
        }
        _ => Some(pick_sentinel(rng)),
    }
}

fn phone_in_format(digits: &str, format: u8) -> String {
    match format {
        0 => phone_masked(digits),
        1 => digits.to_owned(),
        _ => format!("+55{digits}"),
    }
}

fn messy_phone(rng: &mut Rng, digits: &str) -> Option<String> {
    let format = *rng.weighted(&[(0u8, 50), (1, 25), (2, 10), (3, 9), (4, 4), (5, 2)])?;
    match format {
        0..=2 => Some(phone_in_format(digits, format)),
        3 => None,
        4 => Some(pick_sentinel(rng)),
        _ => Some(format!(
            "{} ramal {}",
            phone_masked(digits),
            rng.between(10, 99)
        )),
    }
}

fn messy_email(rng: &mut Rng, email: &str) -> Option<String> {
    let format = *rng.weighted(&[(0u8, 82), (1, 4), (2, 3), (3, 2), (4, 6), (5, 3)])?;
    match format {
        0 => Some(email.to_owned()),
        1 => Some(email.to_uppercase()),
        2 => Some(domain_typo(rng, email)),
        3 => Some(without_at_sign(email)),
        4 => None,
        _ => Some(pick_sentinel(rng)),
    }
}

fn messy_name(rng: &mut Rng, name: &str) -> String {
    match rng.below(100) {
        0..=3 => name.to_uppercase(),
        4 => name.to_lowercase(),
        5 if !name.is_ascii() => mojibake(name),
        6 => pad(rng, name),
        _ => name.to_owned(),
    }
}

fn messy_date(rng: &mut Rng, date: (u32, u32, u32)) -> Option<String> {
    match rng.below(100) {
        0..=59 => Some(date_iso(date)),
        60..=89 => Some(date_br(date)),
        90..=97 => None,
        _ => rng.pick(IMPOSSIBLE_DATES).map(|d| (*d).to_owned()),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VarejoParams {
    clientes: usize,
    duplicados: usize,
    placeholders: usize,
    placeholders_mistos: usize,
    pedidos: usize,
    orfaos: usize,
    itens_max_por_pedido: u64,
}

struct Customer {
    person: Person,
    nome: String,
    cpf: Option<String>,
    email: Option<String>,
    telefone: Option<String>,
}

fn customer_row(rng: &mut Rng, id: i64, customer: &Customer) -> Vec<Value> {
    let person = &customer.person;
    let complement = if rng.chance(300) {
        format!(" apto {}", rng.between(1, 1504))
    } else {
        String::new()
    };
    let cidade = if rng.chance(50) {
        person.city.name.to_uppercase()
    } else {
        person.city.name.to_owned()
    };
    let cep = match rng.below(100) {
        0..=69 => Some(cep_masked(&person.cep)),
        70..=94 => Some(person.cep.clone()),
        _ => None,
    };
    let observacoes = match rng.below(100) {
        0..=59 => None,
        60..=84 => rng.pick(PLAIN_NOTES).map(|n| (*n).to_owned()),
        _ => Some(note_embedding(
            rng,
            &Document::Cpf.masked(&person.cpf),
            &phone_masked(&person.phone),
        )),
    };
    vec![
        Value::Int(id),
        text(customer.nome.clone()),
        maybe_text(customer.cpf.clone()),
        maybe_text(customer.email.clone()),
        maybe_text(customer.telefone.clone()),
        maybe_text(messy_date(rng, person.birth)),
        text(format!("{}{complement}", person.street)),
        text(person.bairro),
        text(cidade),
        text(person.city.uf),
        maybe_text(cep),
        maybe_text(observacoes),
    ]
}

/// The same person typed in again: the CPF in the other format, the name in
/// another case, the phone in another layout.
fn duplicate_of(rng: &mut Rng, original: &Customer) -> Customer {
    let person = Person {
        first_name: original.person.first_name.clone(),
        full_name: original.person.full_name.clone(),
        cpf: original.person.cpf.clone(),
        email: original.person.email.clone(),
        phone: original.person.phone.clone(),
        city: original.person.city,
        bairro: original.person.bairro,
        street: original.person.street.clone(),
        cep: original.person.cep.clone(),
        birth: original.person.birth,
    };
    let cpf = if original.cpf.as_deref().is_some_and(|c| c.contains('.')) {
        text_of(&person.cpf)
    } else {
        Document::Cpf.masked(&person.cpf)
    };
    Customer {
        nome: case_mess(rng, &person.full_name),
        cpf: Some(cpf),
        email: Some(person.email.to_lowercase()),
        telefone: Some(phone_in_format(&person.phone, rng.below(3) as u8)),
        person,
    }
}

fn varejo_clientes_pedidos(seed: u64, raw: &serde_json::Value) -> Result<DatasetDto, GenError> {
    let p: VarejoParams = params(raw)?;
    if p.placeholders >= p.clientes || p.duplicados >= p.clientes || p.orfaos > p.pedidos {
        return Err(GenError(
            "placeholders and duplicates need more customers, orphans more orders".to_owned(),
        ));
    }
    let root = Rng::new(seed);
    let mut rng = root.fork("clientes");

    let mut customers: Vec<Customer> = (0..p.clientes)
        .map(|_| {
            let person = Person::generate(&mut rng);
            Customer {
                nome: messy_name(&mut rng, &person.full_name),
                cpf: messy_cpf(&mut rng, &person.cpf),
                email: messy_email(&mut rng, &person.email),
                telefone: messy_phone(&mut rng, &person.phone),
                person,
            }
        })
        .collect();

    let mut placeholder_rng = root.fork("placeholders");
    let mut placed = 0;
    while placed < p.placeholders {
        let row = placeholder_rng.index(customers.len());
        let customer = &mut customers[row];
        if customer.email.as_deref() == Some(PLACEHOLDER_EMAIL) {
            continue;
        }
        customer.email = Some(PLACEHOLDER_EMAIL.to_owned());
        if placed < p.placeholders_mistos {
            customer.nome = customer.person.full_name.clone();
            customer.cpf = Some(Document::Cpf.masked(&customer.person.cpf));
            customer.telefone = Some(phone_masked(&customer.person.phone));
        } else {
            customer.nome = PLACEHOLDER_NAME.to_owned();
            customer.cpf = Some(PLACEHOLDER_CPF.to_owned());
            customer.telefone = Some(PLACEHOLDER_PHONE.to_owned());
        }
        placed += 1;
    }

    let mut duplicate_rng = root.fork("duplicados");
    let mut duplicated = 0;
    let mut attempts = 0;
    while duplicated < p.duplicados {
        attempts += 1;
        if attempts > 100 * p.clientes {
            return Err(GenError(
                "too few customers with a valid CPF to duplicate".to_owned(),
            ));
        }
        let original = &customers[duplicate_rng.index(p.clientes)];
        let real = original.email.as_deref() != Some(PLACEHOLDER_EMAIL)
            && original
                .cpf
                .as_deref()
                .is_some_and(|c| Document::Cpf.is_valid(c));
        if !real {
            continue;
        }
        let duplicate = duplicate_of(&mut duplicate_rng, original);
        let at = duplicate_rng.index(customers.len() + 1);
        customers.insert(at, duplicate);
        duplicated += 1;
    }

    let mut clientes = TableBuilder::new("clientes")
        .primary_key("id", WireType::Integer)
        .column("nome", WireType::String)
        .column("cpf", WireType::String)
        .column("email", WireType::String)
        .column("telefone", WireType::String)
        .column("data_nascimento", WireType::String)
        .column("endereco", WireType::String)
        .column("bairro", WireType::String)
        .column("cidade", WireType::String)
        .column("uf", WireType::String)
        .column("cep", WireType::String)
        .column("observacoes", WireType::String);
    let mut row_rng = root.fork("clientes-linhas");
    for (index, customer) in customers.iter().enumerate() {
        clientes.push(customer_row(&mut row_rng, index as i64 + 1, customer))?;
    }

    let customer_count = customers.len() as u64;
    let mut order_rng = root.fork("pedidos");
    let mut orphan_rows = Vec::new();
    while orphan_rows.len() < p.orfaos {
        let row = order_rng.index(p.pedidos);
        if !orphan_rows.contains(&row) {
            orphan_rows.push(row);
        }
    }
    let mut pedidos = TableBuilder::new("pedidos")
        .primary_key("id", WireType::Integer)
        .foreign_key("cliente_id", WireType::Integer, "clientes", "id")
        .column("data_pedido", WireType::String)
        .column("status", WireType::String)
        .column("forma_pagamento", WireType::String)
        .column("valor_total", WireType::Float);
    let mut itens = TableBuilder::new("itens_pedido")
        .primary_key("id", WireType::Integer)
        .foreign_key("pedido_id", WireType::Integer, "pedidos", "id")
        .column("sku", WireType::String)
        .column("descricao", WireType::String)
        .column("quantidade", WireType::Integer)
        .column("preco_unitario", WireType::Float);
    for row in 0..p.pedidos {
        let order_id = 10_001 + row as i64;
        let cliente_id = if orphan_rows.contains(&row) {
            90_000 + order_rng.between(1, 9_999) as i64 + customer_count as i64
        } else {
            // Squaring a uniform draw skews orders towards the earliest customers.
            let u = order_rng.below(customer_count);
            (u * u / customer_count.max(1)) as i64 + 1
        };
        let date = (
            order_rng.between(2021, 2024) as u32,
            order_rng.between(1, 12) as u32,
            order_rng.between(1, 28) as u32,
        );
        let data_pedido = match order_rng.below(100) {
            0..=69 => date_iso(date),
            70..=98 => date_br(date),
            _ => order_rng
                .pick(IMPOSSIBLE_DATES)
                .copied()
                .unwrap_or("2023-02-30")
                .to_owned(),
        };
        let mut total_cents = 0;
        for _ in 0..order_rng.between(1, p.itens_max_por_pedido) {
            let product = order_rng.index(PRODUCTS.len());
            let (description, price) = PRODUCTS[product];
            let quantity = order_rng
                .weighted(&[(1u64, 70), (2, 20), (3, 7), (5, 3)])
                .copied()
                .unwrap_or(1);
            total_cents += price * quantity;
            itens.push(vec![
                Value::Int(itens.len() as i64 + 1),
                Value::Int(order_id),
                text(format!("SKU-{:05}", 1000 + product * 37)),
                text(description),
                Value::Int(quantity as i64),
                Value::Float(price as f64 / 100.0),
            ])?;
        }
        let shipping = if order_rng.chance(400) {
            0
        } else {
            order_rng.between(990, 4990)
        };
        let status = order_rng
            .weighted(ORDER_STATUS)
            .copied()
            .unwrap_or("entregue");
        let payment = order_rng.weighted(PAYMENT).copied().unwrap_or("pix");
        pedidos.push(vec![
            Value::Int(order_id),
            Value::Int(cliente_id),
            text(data_pedido),
            text(status),
            text(payment),
            Value::Float((total_cents + shipping) as f64 / 100.0),
        ])?;
    }

    Ok(DatasetDto {
        tables: vec![clientes.build(), pedidos.build(), itens.build()],
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FornecedoresParams {
    fornecedores: usize,
    contatos_max_por_fornecedor: u64,
    placeholders_contatos: usize,
}

/// The CNPJ column's mix: masked, bare digits, an integer export that lost
/// its leading zeros, wrong check digits, null, and the `ISENTO` sentinel.
fn messy_cnpj(rng: &mut Rng, digits: &mut [u8]) -> Option<String> {
    let format = *rng.weighted(&[(0u8, 50), (1, 25), (2, 12), (3, 5), (4, 5), (5, 3)])?;
    match format {
        0 => Some(Document::Cnpj.masked(digits)),
        1 => Some(text_of(digits)),
        2 => {
            digits[0] = 0;
            let check = Document::Cnpj.check_digits(&digits[..12]);
            digits[12..].copy_from_slice(&check);
            Some(without_leading_zeros(digits))
        }
        3 => Some(Document::Cnpj.masked(&Document::Cnpj.generate_invalid(rng))),
        4 => None,
        _ => Some("ISENTO".to_owned()),
    }
}

fn fornecedores_b2b(seed: u64, raw: &serde_json::Value) -> Result<DatasetDto, GenError> {
    let p: FornecedoresParams = params(raw)?;
    let root = Rng::new(seed);

    let mut estados = TableBuilder::new("estados")
        .primary_key("sigla", WireType::String)
        .column("nome", WireType::String)
        .column("regiao", WireType::String);
    for (sigla, nome, regiao) in STATES {
        estados.push(vec![text(*sigla), text(*nome), text(*regiao)])?;
    }

    let mut fornecedores = TableBuilder::new("fornecedores")
        .primary_key("id", WireType::Integer)
        .column("cnpj", WireType::String)
        .column("razao_social", WireType::String)
        .column("nome_fantasia", WireType::String)
        .column("email_comercial", WireType::String)
        .column("telefone", WireType::String)
        .column("cep", WireType::String)
        .foreign_key("uf", WireType::String, "estados", "sigla")
        .column("ativo", WireType::Boolean);
    let mut contatos = TableBuilder::new("contatos")
        .primary_key("id", WireType::Integer)
        .foreign_key("fornecedor_id", WireType::Integer, "fornecedores", "id")
        .column("nome", WireType::String)
        .column("cpf", WireType::String)
        .column("email", WireType::String)
        .column("cargo", WireType::String)
        .column("telefone", WireType::String);

    let mut rng = root.fork("fornecedores");
    let mut contact_rng = root.fork("contatos");
    for index in 0..p.fornecedores {
        let mut company = Company::generate(&mut rng);
        let id = index as i64 + 1;
        let cnpj = messy_cnpj(&mut rng, &mut company.cnpj);
        let telefone = if rng.chance(30) {
            format!("0800 {} {:04}", rng.between(100, 999), rng.between(0, 9999))
        } else {
            phone_in_format(&company.phone, if rng.chance(700) { 0 } else { 1 })
        };
        let email = match rng.below(100) {
            0..=89 => Some(company.email.clone()),
            90..=91 => Some(domain_typo(&mut rng, &company.email)),
            _ => None,
        };
        let cep = if rng.chance(750) {
            cep_masked(&company.cep)
        } else {
            company.cep.clone()
        };
        fornecedores.push(vec![
            Value::Int(id),
            maybe_text(cnpj),
            text(company.razao_social.clone()),
            maybe_text((!rng.chance(100)).then(|| company.nome_fantasia.clone())),
            maybe_text(email),
            text(telefone),
            text(cep),
            text(company.city.uf),
            Value::Bool(rng.chance(900)),
        ])?;

        let domain = company.email.rsplit_once('@').map(|(_, d)| d.to_owned());
        for _ in 0..contact_rng.between(1, p.contatos_max_por_fornecedor) {
            let person = Person::generate(&mut contact_rng);
            let cpf = match contact_rng.below(100) {
                0..=59 => Some(Document::Cpf.masked(&person.cpf)),
                60..=84 => Some(text_of(&person.cpf)),
                _ => None,
            };
            let email = match (&domain, contact_rng.chance(600)) {
                (Some(domain), true) => {
                    let local = person.email.split('@').next().unwrap_or("contato");
                    format!("{local}@{domain}")
                }
                _ => person.email.clone(),
            };
            let cargo = contact_rng
                .weighted(JOB_TITLES)
                .copied()
                .unwrap_or("Comprador");
            contatos.push(vec![
                Value::Int(contatos.len() as i64 + 1),
                Value::Int(id),
                text(person.full_name.clone()),
                maybe_text(cpf),
                text(email),
                text(cargo),
                text(phone_masked(&person.phone)),
            ])?;
        }
    }

    let mut placeholder_rng = root.fork("placeholders");
    let total = fornecedores.len() as u64;
    for _ in 0..p.placeholders_contatos {
        contatos.push(vec![
            Value::Int(contatos.len() as i64 + 1),
            Value::Int(placeholder_rng.between(1, total) as i64),
            text(PLACEHOLDER_NAME),
            text(PLACEHOLDER_CPF),
            text(PLACEHOLDER_EMAIL),
            text("Comprador"),
            text(PLACEHOLDER_PHONE),
        ])?;
    }

    Ok(DatasetDto {
        tables: vec![estados.build(), fornecedores.build(), contatos.build()],
    })
}
