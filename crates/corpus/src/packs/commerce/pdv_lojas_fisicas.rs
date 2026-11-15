//! Point-of-sale records of a store chain: stores (most are branches of one
//! company and share its CNPJ root, a few are franchises with their own),
//! cashiers, and consumer-receipt sales. Each sale carries the 44-digit
//! NFC-e access key, which embeds the issuing store's CNPJ, and an
//! optional `cpf_na_nota` typed in at the till by returning customers.

use anonymizer::Value;
use serde::Deserialize;
use server::http::dto::{DatasetDto, WireType};

use super::{cpf_in_either_layout, day, timestamp, weighted};
use crate::generate::company::Company;
use crate::generate::docs::{Document, text_of};
use crate::generate::mess::date_iso;
use crate::generate::person::{Person, cep_masked, weighted_city};
use crate::generate::rng::Rng;
use crate::generate::table::{TableBuilder, maybe_text, text};
use crate::generate::vocab::BAIRROS;
use crate::generate::{GenError, params};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PdvParams {
    lojas: usize,
    /// Per mille of stores that are franchises with their own CNPJ.
    franquias_por_mil: u64,
    operadores_max_por_loja: u64,
    vendas: usize,
    /// Returning customers whose CPFs are typed into receipts.
    clientes_recorrentes: usize,
    /// Per mille of sales with a CPF on the receipt.
    cpf_na_nota_por_mil: u64,
}

/// IBGE codes of the federative units, the first two digits of an access
/// key.
const UF_CODES: &[(&str, u8)] = &[
    ("RO", 11),
    ("AC", 12),
    ("AM", 13),
    ("RR", 14),
    ("PA", 15),
    ("AP", 16),
    ("TO", 17),
    ("MA", 21),
    ("PI", 22),
    ("CE", 23),
    ("RN", 24),
    ("PB", 25),
    ("PE", 26),
    ("AL", 27),
    ("SE", 28),
    ("BA", 29),
    ("MG", 31),
    ("ES", 32),
    ("RJ", 33),
    ("SP", 35),
    ("PR", 41),
    ("SC", 42),
    ("RS", 43),
    ("MS", 50),
    ("MT", 51),
    ("GO", 52),
    ("DF", 53),
];

const PAYMENT: &[(&str, u64)] = &[
    ("cartao_debito", 34),
    ("cartao_credito", 30),
    ("pix", 24),
    ("dinheiro", 10),
    ("vale_alimentacao", 2),
];

/// The access key's check digit: mod 11 with weights 2 to 9 cycling from
/// the rightmost digit; a remainder below 2 gives 0.
fn access_key_check_digit(digits: &str) -> u32 {
    let sum: u32 = digits
        .bytes()
        .rev()
        .zip((2..=9).cycle())
        .map(|(byte, weight)| u32::from(byte - b'0') * weight)
        .sum();
    let rest = sum % 11;
    if rest < 2 { 0 } else { 11 - rest }
}

/// An NFC-e access key: state code, year and month, issuer CNPJ, model 65,
/// series, receipt number, emission type, random code and check digit.
fn access_key(
    rng: &mut Rng,
    uf: &str,
    (year, month, _): (u32, u32, u32),
    cnpj: &[u8],
    series: u64,
    number: u64,
) -> String {
    let state = UF_CODES
        .iter()
        .find(|(code, _)| *code == uf)
        .map_or(35, |(_, ibge)| *ibge);
    let body = format!(
        "{state:02}{:02}{month:02}{}65{series:03}{number:09}1{:08}",
        year % 100,
        text_of(cnpj),
        rng.below(100_000_000)
    );
    format!("{body}{}", access_key_check_digit(&body))
}

struct Store {
    cnpj: Vec<u8>,
    uf: &'static str,
}

pub fn generate(seed: u64, raw: &serde_json::Value) -> Result<DatasetDto, GenError> {
    let p: PdvParams = params(raw)?;
    if p.lojas == 0 || p.clientes_recorrentes == 0 {
        return Err(GenError("sales need stores and customers".to_owned()));
    }
    let root = Rng::new(seed);

    let mut lojas = TableBuilder::new("lojas")
        .primary_key("id", WireType::Integer)
        .column("cnpj", WireType::String)
        .column("razao_social", WireType::String)
        .column("nome_fantasia", WireType::String)
        .column("cidade", WireType::String)
        .column("uf", WireType::String)
        .column("cep", WireType::String);
    let mut rng = root.fork("lojas");
    let chain = Company::generate(&mut rng);
    let mut stores = Vec::with_capacity(p.lojas);
    for index in 0..p.lojas {
        let city = weighted_city(&mut rng);
        let bairro = rng.pick(BAIRROS).copied().unwrap_or("Centro");
        let (cnpj, razao_social) = if index > 0 && rng.chance(p.franquias_por_mil) {
            let franchise = Company::generate(&mut rng);
            (franchise.cnpj, franchise.razao_social)
        } else {
            let mut digits = chain.cnpj[..8].to_vec();
            let branch = index as u64 + 1;
            digits.extend(
                [
                    branch / 1000,
                    branch / 100 % 10,
                    branch / 10 % 10,
                    branch % 10,
                ]
                .map(|d| d as u8),
            );
            let check = Document::Cnpj.check_digits(&digits);
            digits.extend(check);
            (digits, chain.razao_social.clone())
        };
        let cep = format!(
            "{:05}{:03}",
            rng.between(u64::from(city.cep.0), u64::from(city.cep.1)),
            rng.below(1000)
        );
        lojas.push(vec![
            Value::Int(index as i64 + 1),
            text(Document::Cnpj.masked(&cnpj)),
            text(razao_social),
            text(format!("{} {bairro}", chain.nome_fantasia)),
            text(city.name),
            text(city.uf),
            text(cep_masked(&cep)),
        ])?;
        stores.push(Store { cnpj, uf: city.uf });
    }

    let mut operadores = TableBuilder::new("operadores")
        .primary_key("id", WireType::Integer)
        .foreign_key("loja_id", WireType::Integer, "lojas", "id")
        .column("matricula", WireType::String)
        .column("nome", WireType::String)
        .column("cpf", WireType::String)
        .column("admitido_em", WireType::String);
    let mut rng = root.fork("operadores");
    let mut cashiers_by_store: Vec<Vec<i64>> = Vec::with_capacity(stores.len());
    for store in 0..stores.len() {
        let mut cashiers = Vec::new();
        for _ in 0..rng.between(1, p.operadores_max_por_loja) {
            let id = operadores.len() as i64 + 1;
            let person = Person::generate(&mut rng);
            operadores.push(vec![
                Value::Int(id),
                Value::Int(store as i64 + 1),
                text(format!("OP{:03}{:03}", store + 1, cashiers.len() + 1)),
                text(person.full_name),
                text(Document::Cpf.masked(&person.cpf)),
                text(date_iso(day(&mut rng, 2015, 2024))),
            ])?;
            cashiers.push(id);
        }
        cashiers_by_store.push(cashiers);
    }

    let mut customer_rng = root.fork("clientes");
    let regulars: Vec<Vec<u8>> = (0..p.clientes_recorrentes)
        .map(|_| Document::Cpf.generate(&mut customer_rng))
        .collect();

    let mut vendas = TableBuilder::new("vendas")
        .primary_key("id", WireType::Integer)
        .foreign_key("loja_id", WireType::Integer, "lojas", "id")
        .foreign_key("operador_id", WireType::Integer, "operadores", "id")
        .column("emitida_em", WireType::String)
        .column("numero_cupom", WireType::Integer)
        .column("chave_nfe", WireType::String)
        .column("valor_total", WireType::Float)
        .column("forma_pagamento", WireType::String)
        .column("cpf_na_nota", WireType::String);
    let mut rng = root.fork("vendas");
    let mut next_number = vec![1_u64; stores.len()];
    for row in 0..p.vendas {
        // Squaring a uniform draw gives the flagship stores most of the sales.
        let u = rng.below(stores.len() as u64);
        let store_index = (u * u / stores.len() as u64) as usize;
        let store = &stores[store_index];
        let cashier = rng
            .pick(&cashiers_by_store[store_index])
            .copied()
            .unwrap_or(1);
        let date = day(&mut rng, 2024, 2024);
        let number = next_number[store_index];
        next_number[store_index] += rng.between(1, 5);
        let key = access_key(&mut rng, store.uf, date, &store.cnpj, 1, number);
        let cpf = rng.chance(p.cpf_na_nota_por_mil).then(|| {
            // A squared draw makes a few regulars ask for a CPF on most receipts.
            let r = rng.below(regulars.len() as u64);
            let regular = &regulars[(r * r / regulars.len() as u64) as usize];
            cpf_in_either_layout(&mut rng, regular, 550)
        });
        let cents = rng.between(390, 25_000) + rng.below(4) * rng.below(30_000);
        vendas.push(vec![
            Value::Int(row as i64 + 1),
            Value::Int(store_index as i64 + 1),
            Value::Int(cashier),
            text(timestamp(&mut rng, date)),
            Value::Int(number as i64),
            text(key),
            Value::Float(cents as f64 / 100.0),
            text(weighted(&mut rng, PAYMENT)),
            maybe_text(cpf),
        ])?;
    }

    Ok(DatasetDto {
        tables: vec![lojas.build(), operadores.build(), vendas.build()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_access_key_has_44_digits_and_embeds_the_issuer_cnpj() {
        let mut rng = Rng::new(8);
        let cnpj = Document::Cnpj.generate(&mut rng);
        let key = access_key(&mut rng, "RJ", (2024, 3, 9), &cnpj, 1, 4321);
        assert_eq!(key.len(), 44);
        assert!(key.bytes().all(|b| b.is_ascii_digit()));
        assert!(key.starts_with("332403"));
        assert_eq!(&key[6..20], text_of(&cnpj));
        assert_eq!(&key[20..22], "65");
        let check = access_key_check_digit(&key[..43]);
        assert_eq!(key[43..].parse::<u32>().ok(), Some(check));
    }

    #[test]
    fn the_check_digit_follows_the_mod_eleven_rule() {
        // 43 ones: weights 2..9 cycle, sum = 5 * 44 + (2+3+4) = 229; 229 % 11 = 9; 11 - 9 = 2.
        assert_eq!(access_key_check_digit(&"1".repeat(43)), 2);
    }
}
