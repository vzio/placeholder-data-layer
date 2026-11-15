//! The `legacy` pack: dumps of old, messy and adversarial systems — a
//! Latin-1 ERP, a spreadsheet imported as text, a staging database with QA
//! rows mixed in, composite keys and foreign key cycles, generic
//! entity-attribute-value columns. Its curated adversarial scenario commits
//! its dataset and registers no generator.

mod chaves;
mod eav;
mod erp;
mod homologacao;
mod legacy_text;
mod planilha;

use crate::generate::GeneratorFn;

pub fn generator(name: &str) -> Option<GeneratorFn> {
    match name {
        "erp_legado_latin1" => Some(erp::erp_legado_latin1),
        "planilha_importada_excel" => Some(planilha::planilha_importada_excel),
        "ambiente_homologacao_misturado" => Some(homologacao::ambiente_homologacao_misturado),
        "chaves_compostas_e_ciclos" => Some(chaves::chaves_compostas_e_ciclos),
        "colunas_genericas_eav" => Some(eav::colunas_genericas_eav),
        _ => None,
    }
}
