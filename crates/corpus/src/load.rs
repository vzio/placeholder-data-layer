//! Finding, reading and materialising scenarios. Each scenario's dataset is
//! loaded once per process and shared.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use anonymizer::Dataset;
use server::http::dto::DatasetDto;

use crate::CorpusError;
use crate::scenario::{DatasetSource, ScenarioFile};

/// A scenario with its source dataset in both shapes.
#[derive(Debug)]
pub struct Loaded {
    pub file: ScenarioFile,
    pub wire: DatasetDto,
    pub source: Dataset,
}

pub fn scenarios_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/scenarios")
}

/// Every directory under `fixtures/scenarios`, sorted.
pub fn scenario_names() -> Result<Vec<String>, CorpusError> {
    let dir = scenarios_dir();
    let entries = std::fs::read_dir(&dir).map_err(|e| err(format!("{}: {e}", dir.display())))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| err(e.to_string()))?;
        if entry.path().is_dir() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

pub fn read_scenario(name: &str) -> Result<ScenarioFile, CorpusError> {
    let path = scenarios_dir().join(name).join("expected.json");
    let text =
        std::fs::read_to_string(&path).map_err(|e| err(format!("{}: {e}", path.display())))?;
    serde_json::from_str(&text).map_err(|e| err(format!("{}: {e}", path.display())))
}

/// The scenario's wire dataset: read from disk, or regenerated.
pub fn materialise(name: &str, source: &DatasetSource) -> Result<DatasetDto, CorpusError> {
    match source {
        DatasetSource::File { path } => {
            let path = scenarios_dir().join(name).join(path);
            let text = std::fs::read_to_string(&path)
                .map_err(|e| err(format!("{}: {e}", path.display())))?;
            serde_json::from_str(&text).map_err(|e| err(format!("{}: {e}", path.display())))
        }
        DatasetSource::Generator {
            name: generator,
            seed,
            params,
            ..
        } => {
            let generator_fn = crate::generate::generator(generator)
                .ok_or_else(|| err(format!("no pack registers generator `{generator}`")))?;
            generator_fn(*seed, params).map_err(|e| err(format!("generator `{generator}`: {e}")))
        }
    }
}

/// `blake3:<hex>` of the compact JSON serialisation.
pub fn digest(dataset: &DatasetDto) -> Result<String, CorpusError> {
    let bytes = serde_json::to_vec(dataset).map_err(|e| err(e.to_string()))?;
    Ok(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
}

/// The first `rows` rows of every table.
pub fn preview(dataset: &DatasetDto, rows: usize) -> DatasetDto {
    let mut preview = dataset.clone();
    for table in &mut preview.tables {
        table.rows.truncate(rows);
    }
    preview
}

pub fn read_preview(name: &str) -> Result<DatasetDto, CorpusError> {
    let path = scenarios_dir().join(name).join("preview.json");
    let text =
        std::fs::read_to_string(&path).map_err(|e| err(format!("{}: {e}", path.display())))?;
    serde_json::from_str(&text).map_err(|e| err(format!("{}: {e}", path.display())))
}

type Slot = Arc<OnceLock<Result<Arc<Loaded>, CorpusError>>>;

/// The scenario, loaded at most once per process.
pub fn load(name: &str) -> Result<Arc<Loaded>, CorpusError> {
    static CACHE: OnceLock<Mutex<BTreeMap<String, Slot>>> = OnceLock::new();
    let slot = {
        let mut cache = CACHE
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| err("scenario cache poisoned"))?;
        Arc::clone(cache.entry(name.to_owned()).or_default())
    };
    slot.get_or_init(|| {
        let file = read_scenario(name)?;
        let wire = materialise(name, &file.dataset)?;
        let source = wire
            .clone()
            .into_dataset()
            .map_err(|e| err(format!("{name}: the dataset is rejected: {e}")))?;
        Ok(Arc::new(Loaded { file, wire, source }))
    })
    .clone()
}

fn err(message: impl Into<String>) -> CorpusError {
    CorpusError(message.into())
}
