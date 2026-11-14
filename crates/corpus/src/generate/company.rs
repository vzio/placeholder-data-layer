//! A coherent synthetic company. A small share are sole traders (MEI), whose
//! legal name is by convention the owner's name followed by the owner's CPF.

use super::docs::{Document, text_of};
use super::person::{Person, ascii_fold, cep_for, email_for, phone_for, weighted_city};
use super::rng::Rng;
use super::vocab::{COMPANY_SECTORS, COMPANY_STEMS, COMPANY_SUFFIXES, City};

pub struct Company {
    pub cnpj: Vec<u8>,
    pub razao_social: String,
    pub nome_fantasia: String,
    pub email: String,
    /// Area code plus subscriber number, digits only.
    pub phone: String,
    pub city: &'static City,
    /// Eight digits.
    pub cep: String,
    /// The owner's CPF when the legal name embeds it.
    pub owner_cpf: Option<Vec<u8>>,
}

impl Company {
    pub fn generate(rng: &mut Rng) -> Company {
        let stem = rng.pick(COMPANY_STEMS).copied().unwrap_or("Aurora");
        let sector = rng.pick(COMPANY_SECTORS).copied().unwrap_or("Comércio");
        let city = weighted_city(rng);
        let cnpj = Document::Cnpj.generate(rng);
        let slug = ascii_fold(stem).to_lowercase();
        if rng.chance(100) {
            let owner = Person::generate(rng);
            let razao_social =
                format!("{} {}", owner.full_name.to_uppercase(), text_of(&owner.cpf));
            return Company {
                cnpj,
                razao_social,
                nome_fantasia: format!("{stem} {sector}"),
                email: email_for(rng, &owner.first_name, &slug, owner.birth.0),
                phone: phone_for(rng, city.ddd),
                cep: cep_for(rng, city),
                city,
                owner_cpf: Some(owner.cpf),
            };
        }
        let suffix = rng.weighted(COMPANY_SUFFIXES).copied().unwrap_or("LTDA");
        let mut razao_social = format!("{stem} {sector} {suffix}");
        if rng.chance(400) {
            razao_social = razao_social.to_uppercase();
        }
        let mailbox = *rng
            .pick(&[
                "contato",
                "vendas",
                "comercial",
                "financeiro",
                "atendimento",
            ])
            .unwrap_or(&"contato");
        let tld = if rng.chance(800) { "com.br" } else { "com" };
        let sector_slug = ascii_fold(sector).to_lowercase();
        Company {
            cnpj,
            razao_social,
            nome_fantasia: stem.to_owned(),
            email: format!("{mailbox}@{slug}{sector_slug}.{tld}"),
            phone: landline_or_mobile(rng, city.ddd),
            cep: cep_for(rng, city),
            city,
            owner_cpf: None,
        }
    }
}

fn landline_or_mobile(rng: &mut Rng, ddd: u8) -> String {
    if rng.chance(600) {
        format!("{ddd}{}{:07}", rng.between(2, 4), rng.between(0, 9_999_999))
    } else {
        phone_for(rng, ddd)
    }
}
