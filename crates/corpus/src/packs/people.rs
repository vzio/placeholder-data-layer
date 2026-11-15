//! The `people` pack: databases about people in their working, clinical,
//! financial and school lives, each carrying identifiers the shared
//! generators do not issue (PIS/PASEP, CNS, RENAVAM, state RGs, plates,
//! chassis numbers, PIX keys) and the sensitive free text that comes with
//! them.

mod banco;
mod clinica;
mod condominio;
mod escola;
mod folha;
mod ids;
mod seguros;
mod words;

use crate::generate::GeneratorFn;
use crate::generate::rng::Rng;

pub fn generator(name: &str) -> Option<GeneratorFn> {
    match name {
        "folha_pagamento_rh" => Some(folha::generate),
        "clinica_prontuarios" => Some(clinica::generate),
        "banco_contas_transacoes" => Some(banco::generate),
        "escola_matriculas" => Some(escola::generate),
        "seguros_apolices_sinistros" => Some(seguros::generate),
        "condominio_moradores" => Some(condominio::generate),
        _ => None,
    }
}

/// A calendar date with the day capped at 28, so every month holds it.
fn date_between(rng: &mut Rng, first_year: u32, last_year: u32) -> (u32, u32, u32) {
    (
        rng.between(u64::from(first_year), u64::from(last_year)) as u32,
        rng.between(1, 12) as u32,
        rng.between(1, 28) as u32,
    )
}

/// An amount in cents with a heavy right tail: a decade is drawn by weight,
/// then a value spread evenly over a few steps inside it. Integer arithmetic
/// only, so the bytes never depend on a platform's floating-point library.
fn heavy_tailed_cents(rng: &mut Rng, decades: &[(u64, u64)]) -> u64 {
    let low = rng.weighted(decades).copied().unwrap_or(100);
    let step = rng.between(1, 9);
    let within = rng.below(low.max(1));
    low * step + within
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scenario_name_has_a_generator() {
        for name in [
            "folha_pagamento_rh",
            "clinica_prontuarios",
            "banco_contas_transacoes",
            "escola_matriculas",
            "seguros_apolices_sinistros",
            "condominio_moradores",
        ] {
            assert!(generator(name).is_some(), "{name}");
        }
        assert!(generator("nope").is_none());
    }

    #[test]
    fn heavy_tailed_amounts_stay_inside_their_decades() {
        let mut rng = Rng::new(1);
        let decades = [(100, 50), (10_000, 45), (1_000_000, 5)];
        let draws: Vec<u64> = (0..2000)
            .map(|_| heavy_tailed_cents(&mut rng, &decades))
            .collect();
        assert!(draws.iter().all(|d| (100..10_000_000).contains(d)));
        assert!(draws.iter().any(|d| *d >= 1_000_000));
    }
}
