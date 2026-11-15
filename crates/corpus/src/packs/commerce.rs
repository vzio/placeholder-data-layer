//! The commerce pack: a marketplace, a CRM lead funnel, a SaaS billing
//! database, a loyalty programme, a customer-service ticket system and the
//! point-of-sale records of a chain of physical stores.

mod assinaturas_saas;
mod atendimento_sac_tickets;
mod crm_leads_funil;
mod ecommerce_marketplace;
mod pdv_lojas_fisicas;
mod programa_fidelidade;

use crate::generate::GeneratorFn;
use crate::generate::docs::{Document, text_of};
use crate::generate::rng::Rng;

pub fn generator(name: &str) -> Option<GeneratorFn> {
    match name {
        "ecommerce_marketplace" => Some(ecommerce_marketplace::generate),
        "crm_leads_funil" => Some(crm_leads_funil::generate),
        "assinaturas_saas" => Some(assinaturas_saas::generate),
        "programa_fidelidade" => Some(programa_fidelidade::generate),
        "atendimento_sac_tickets" => Some(atendimento_sac_tickets::generate),
        "pdv_lojas_fisicas" => Some(pdv_lojas_fisicas::generate),
        _ => None,
    }
}

/// A uniformly drawn item of a non-empty word list.
fn pick(rng: &mut Rng, items: &[&'static str]) -> &'static str {
    rng.pick(items).copied().unwrap_or("")
}

/// An item drawn with probability proportional to its weight.
fn weighted(rng: &mut Rng, items: &[(&'static str, u64)]) -> &'static str {
    rng.weighted(items).copied().unwrap_or("")
}

/// A calendar day in `first_year..=last_year`; days stop at 28 so every
/// month is valid.
fn day(rng: &mut Rng, first_year: u64, last_year: u64) -> (u32, u32, u32) {
    (
        rng.between(first_year, last_year) as u32,
        rng.between(1, 12) as u32,
        rng.between(1, 28) as u32,
    )
}

/// `YYYY-MM-DD HH:MM:SS` within business hours.
fn timestamp(rng: &mut Rng, (year, month, day): (u32, u32, u32)) -> String {
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        rng.between(8, 21),
        rng.between(0, 59),
        rng.between(0, 59)
    )
}

/// A lowercase, hyphenated, version-4 UUID.
fn uuid(rng: &mut Rng) -> String {
    let high = rng.next_u64();
    let low = rng.next_u64();
    let time_high = (high & 0x0fff) | 0x4000;
    let clock = ((low >> 48) & 0x3fff) | 0x8000;
    format!(
        "{:08x}-{:04x}-{time_high:04x}-{clock:04x}-{:012x}",
        high >> 32,
        (high >> 16) & 0xffff,
        low & 0xffff_ffff_ffff
    )
}

/// A CPF masked most of the time and as bare digits otherwise.
fn cpf_in_either_layout(rng: &mut Rng, digits: &[u8], masked_per_mille: u64) -> String {
    if rng.chance(masked_per_mille) {
        Document::Cpf.masked(digits)
    } else {
        text_of(digits)
    }
}

/// Eleven digits that fail the CPF check-digit rule.
fn eleven_digits_failing_cpf(mut candidate: impl FnMut() -> String) -> String {
    loop {
        let digits = candidate();
        if !Document::Cpf.is_valid(&digits) {
            return digits;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uuid_has_the_hyphenated_version_four_layout() {
        let mut rng = Rng::new(3);
        for _ in 0..50 {
            let id = uuid(&mut rng);
            let groups: Vec<usize> = id.split('-').map(str::len).collect();
            assert_eq!(groups, vec![8, 4, 4, 4, 12]);
            assert_eq!(id.as_bytes()[14], b'4');
            assert!(matches!(id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
        }
    }

    #[test]
    fn eleven_digits_failing_cpf_never_validate() {
        let mut rng = Rng::new(4);
        for _ in 0..200 {
            let digits =
                eleven_digits_failing_cpf(|| format!("{:011}", rng.below(100_000_000_000)));
            assert!(!Document::Cpf.is_valid(&digits));
        }
    }
}
