//! A coherent synthetic person: the email is built from the name, and the
//! phone's area code, the CEP and the state all agree with the city.

use super::docs::Document;
use super::rng::Rng;
use super::vocab::{
    BAIRROS, CITIES, City, EMAIL_PROVIDERS, FIRST_NAMES_FEMALE, FIRST_NAMES_MALE, STREET_KINDS,
    STREET_NAMES, SURNAMES,
};

pub struct Person {
    pub first_name: String,
    pub full_name: String,
    pub cpf: Vec<u8>,
    pub email: String,
    /// Area code plus subscriber number, digits only.
    pub phone: String,
    pub city: &'static City,
    pub bairro: &'static str,
    pub street: String,
    /// Eight digits.
    pub cep: String,
    pub birth: (u32, u32, u32),
}

impl Person {
    pub fn generate(rng: &mut Rng) -> Person {
        let female = rng.chance(510);
        let pool = if female {
            FIRST_NAMES_FEMALE
        } else {
            FIRST_NAMES_MALE
        };
        let first_name = rng.pick(pool).copied().unwrap_or("Ana").to_owned();
        let surname_count = 1 + rng.below(3) as usize;
        let mut surnames: Vec<&str> = Vec::with_capacity(surname_count);
        while surnames.len() < surname_count {
            let surname = rng.pick(SURNAMES).copied().unwrap_or("Silva");
            if !surnames.contains(&surname) {
                surnames.push(surname);
            }
        }
        let connector = if surname_count > 1 && rng.chance(200) {
            " da "
        } else {
            " "
        };
        let full_name = format!("{first_name}{connector}{}", surnames.join(" "));
        let last = surnames.last().copied().unwrap_or("Silva");
        let city = weighted_city(rng);
        let birth = (
            rng.between(1945, 2006) as u32,
            rng.between(1, 12) as u32,
            rng.between(1, 28) as u32,
        );
        Person {
            email: email_for(rng, &first_name, last, birth.0),
            phone: phone_for(rng, city.ddd),
            cep: cep_for(rng, city),
            street: street_for(rng),
            bairro: rng.pick(BAIRROS).copied().unwrap_or("Centro"),
            cpf: Document::Cpf.generate(rng),
            first_name,
            full_name,
            city,
            birth,
        }
    }
}

pub fn weighted_city(rng: &mut Rng) -> &'static City {
    let total: u64 = CITIES.iter().map(|c| c.weight).sum();
    let mut draw = rng.below(total);
    for city in CITIES {
        if draw < city.weight {
            return city;
        }
        draw -= city.weight;
    }
    &CITIES[0]
}

pub fn email_for(rng: &mut Rng, first: &str, last: &str, year: u32) -> String {
    let first = ascii_fold(first).to_lowercase();
    let last = ascii_fold(last).to_lowercase();
    let local = match rng.below(6) {
        0 => format!("{first}.{last}"),
        1 => format!("{first}{last}"),
        2 => format!("{first}_{last}"),
        3 => format!("{first}.{last}{}", rng.between(1, 99)),
        4 => format!("{}{last}{:02}", first.get(..1).unwrap_or("x"), year % 100),
        _ => format!("{first}{}", rng.between(100, 9999)),
    };
    let domain = rng
        .weighted(EMAIL_PROVIDERS)
        .copied()
        .unwrap_or("gmail.com");
    format!("{local}@{domain}")
}

/// A mobile (`9` and eight digits) or, less often, a landline number.
pub fn phone_for(rng: &mut Rng, ddd: u8) -> String {
    if rng.chance(850) {
        format!("{ddd}9{:08}", rng.between(60_000_000, 99_999_999))
    } else {
        format!("{ddd}{}{:07}", rng.between(2, 5), rng.between(0, 9_999_999))
    }
}

pub fn cep_for(rng: &mut Rng, city: &City) -> String {
    format!(
        "{:05}{:03}",
        rng.between(u64::from(city.cep.0), u64::from(city.cep.1)),
        rng.between(0, 999)
    )
}

pub fn street_for(rng: &mut Rng) -> String {
    let kind = rng.weighted(STREET_KINDS).copied().unwrap_or("Rua");
    let name = rng.pick(STREET_NAMES).copied().unwrap_or("Brasil");
    format!("{kind} {name}, {}", rng.between(1, 3999))
}

pub fn phone_masked(digits: &str) -> String {
    let (ddd, number) = digits.split_at(2.min(digits.len()));
    let split = number.len().saturating_sub(4);
    format!("({ddd}) {}-{}", &number[..split], &number[split..])
}

pub fn cep_masked(digits: &str) -> String {
    let split = 5.min(digits.len());
    format!("{}-{}", &digits[..split], &digits[split..])
}

/// Drops Portuguese diacritics; anything else non-ASCII is dropped.
pub fn ascii_fold(text: &str) -> String {
    text.chars()
        .filter_map(|c| match c {
            'á' | 'à' | 'â' | 'ã' => Some('a'),
            'Á' | 'À' | 'Â' | 'Ã' => Some('A'),
            'é' | 'ê' => Some('e'),
            'É' | 'Ê' => Some('E'),
            'í' => Some('i'),
            'Í' => Some('I'),
            'ó' | 'ô' | 'õ' => Some('o'),
            'Ó' | 'Ô' | 'Õ' => Some('O'),
            'ú' | 'ü' => Some('u'),
            'Ú' => Some('U'),
            'ç' => Some('c'),
            'Ç' => Some('C'),
            ' ' | '-' => None,
            c if c.is_ascii() => Some(c),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_person_is_internally_coherent() {
        let mut rng = Rng::new(9);
        for _ in 0..100 {
            let person = Person::generate(&mut rng);
            assert!(person.phone.starts_with(&person.city.ddd.to_string()));
            let cep_prefix: u32 = person.cep[..5].parse().unwrap_or(0);
            assert!((person.city.cep.0..=person.city.cep.1).contains(&cep_prefix));
            assert!(Document::Cpf.is_valid(&super::super::docs::text_of(&person.cpf)));
            assert!(person.email.is_ascii() && person.email.contains('@'));
        }
    }

    #[test]
    fn masks_have_the_expected_punctuation() {
        assert_eq!(phone_masked("11987654321"), "(11) 98765-4321");
        assert_eq!(phone_masked("1132654321"), "(11) 3265-4321");
        assert_eq!(cep_masked("01310100"), "01310-100");
    }
}
