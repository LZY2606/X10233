use crate::model::{supported_unit, CasePackage, ObservationKey, RawObservationInput, RevisionPackage};

pub fn validate_case(package: &CasePackage) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if package.schema_version != "carrier-break-case/v1" {
        errors.push("unsupported case schema_version".into());
    }
    if package.case_id.trim().is_empty() {
        errors.push("case_id is required".into());
    }
    if package.station.id.trim().is_empty() {
        errors.push("station.id is required".into());
    }
    if package.epochs.is_empty() {
        errors.push("at least one epoch is required".into());
    }
    if package.satellites.is_empty() {
        errors.push("at least one satellite is required".into());
    }
    if package.observations.is_empty() {
        errors.push("at least one raw observation is required".into());
    }
    let epoch: std::collections::HashSet<&str> = package.epochs.iter().map(|item| item.id.as_str()).collect();
    let satellites: std::collections::HashSet<&str> = package.satellites.iter().map(|item| item.id.as_str()).collect();
    let mut seen = std::collections::HashSet::new();
    for (index, row) in package.observations.iter().enumerate() {
        let where_ = format!("observations[{index}]");
        if !epoch.contains(row.0.as_str()) {
            errors.push(format!("{where_}: unknown epoch_id"));
        }
        if !satellites.contains(row.1.as_str()) {
            errors.push(format!("{where_}: unknown satellite"));
        }
        if row.2.as_deref().unwrap_or_default().trim().is_empty() {
            errors.push(format!("{where_}: observation_code is required"));
        }
        if row.3.unwrap_or_default() <= 0 {
            errors.push(format!("{where_}: positive frequency_number is required"));
        }
        if row.4.as_deref().unwrap_or_default().trim().is_empty() {
            errors.push(format!("{where_}: raw_text is required"));
        }
        if !row.5.as_deref().map(supported_unit).unwrap_or(false) {
            errors.push(format!("{where_}: unit must be m, cycles, Hz, dB-Hz or s"));
        }
        let key = (row.0.clone(), row.1.clone(), row.2.clone(), row.3);
        if !seen.insert(key) {
            errors.push(format!("{where_}: duplicate epoch/satellite/code/frequency"));
        }
    }
    let mut epochs: Vec<&crate::model::EpochDef> = package.epochs.iter().collect();
    epochs.sort_by(|a, b| a.virtual_seconds.partial_cmp(&b.virtual_seconds).unwrap());
    for pair in epochs.windows(2) {
        if pair[0].virtual_seconds >= pair[1].virtual_seconds {
            errors.push("epoch virtual_seconds must increase".into());
        }
    }
    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

pub fn validate_revision(package: &RevisionPackage) -> Result<ObservationKey, Vec<String>> {
    let mut errors = Vec::new();
    if package.schema_version != "carrier-break-revision/v1" {
        errors.push("unsupported revision schema_version".into());
    }
    if package.revision_id.trim().is_empty() {
        errors.push("revision_id is required".into());
    }
    if package.parent_event_id.trim().is_empty() {
        errors.push("parent_event_id is required".into());
    }
    if !matches!(package.kind.as_str(), "parameter_revision" | "source_revision") {
        errors.push("kind must be parameter_revision or source_revision".into());
    }
    if package.kind == "parameter_revision" && package.parameters.is_none() {
        errors.push("parameters are required for parameter_revision".into());
    }
    if package.kind == "source_revision" {
        match &package.observation_key {
            Some(key) => {
                if key.epoch_id.is_empty() || key.satellite.is_empty() || key.observation_code.is_empty() || key.frequency_number <= 0
                {
                    errors.push("observation_key must identify epoch/satellite/code/frequency".into());
                }
                if package.raw_text.is_none() || package.unit.is_none() {
                    errors.push("raw_text and unit are required for source_revision".into());
                } else if !supported_unit(package.unit.as_deref().unwrap()) {
                    errors.push("source_revision uses an unsupported unit".into());
                }
                if errors.is_empty() {
                    return Ok(key.clone());
                }
            }
            None => errors.push("observation_key is required for source_revision".into()),
        }
    }
    Err(errors)
}
