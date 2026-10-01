#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use thiserror::Error;

const SCORE_SCALE: u128 = 1_000_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub group_field: String,
    pub label_field: Option<String>,
    pub folds: usize,
    pub seed: String,
}

impl Config {
    pub fn validate(&self) -> Result<(), HoldoutError> {
        if self.group_field.trim().is_empty() {
            return Err(HoldoutError::InvalidConfig(
                "group field cannot be empty".to_owned(),
            ));
        }
        if self
            .label_field
            .as_ref()
            .is_some_and(|field| field.trim().is_empty())
        {
            return Err(HoldoutError::InvalidConfig(
                "label field cannot be empty".to_owned(),
            ));
        }
        if !(2..=256).contains(&self.folds) {
            return Err(HoldoutError::InvalidConfig(
                "fold count must be between 2 and 256".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoldSummary {
    pub index: usize,
    pub records: usize,
    pub groups: usize,
    pub labels: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub tool_version: String,
    pub input_sha256: String,
    pub config: Config,
    pub records: usize,
    pub groups: usize,
    pub fold_summaries: Vec<FoldSummary>,
    pub assignments: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditReport {
    pub valid: bool,
    pub input_sha256: String,
    pub records: usize,
    pub groups: usize,
    pub violations: Vec<String>,
}

#[derive(Debug, Error)]
pub enum HoldoutError {
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("line {line}: invalid JSON: {source}")]
    InvalidJson {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("line {line}: field `{field}` is missing")]
    MissingField { line: usize, field: String },
    #[error("line {line}: field `{field}` must be a non-null JSON scalar")]
    InvalidScalar { line: usize, field: String },
    #[error("the input contains no JSON records")]
    EmptyInput,
}

#[derive(Debug, Clone)]
struct ParsedRecord {
    raw: String,
    group: String,
    label: Option<String>,
}

#[derive(Debug)]
struct GroupBucket {
    key: String,
    records: Vec<usize>,
    labels: BTreeMap<String, usize>,
    order_hash: [u8; 32],
}

#[derive(Debug, Default)]
struct MutableFold {
    records: usize,
    groups: usize,
    labels: BTreeMap<String, usize>,
}

pub fn plan(input: &[u8], config: Config) -> Result<Manifest, HoldoutError> {
    let parsed = parse_records(input, &config)?;
    Ok(build_plan(input, &parsed, config))
}

pub fn split(input: &[u8], config: Config) -> Result<(Manifest, Vec<Vec<u8>>), HoldoutError> {
    let parsed = parse_records(input, &config)?;
    let manifest = build_plan(input, &parsed, config);
    let mut outputs = vec![Vec::new(); manifest.config.folds];

    for record in parsed {
        let fold = manifest.assignments[&record.group];
        outputs[fold].extend_from_slice(record.raw.as_bytes());
        outputs[fold].push(b'\n');
    }

    Ok((manifest, outputs))
}

pub fn audit(input: &[u8], manifest: &Manifest) -> Result<AuditReport, HoldoutError> {
    let parsed = parse_records(input, &manifest.config)?;
    let rebuilt = build_plan(input, &parsed, manifest.config.clone());
    let mut violations = Vec::new();

    if manifest.schema_version != 1 {
        violations.push(format!(
            "unsupported manifest schema version {}",
            manifest.schema_version
        ));
    }
    if manifest.input_sha256 != rebuilt.input_sha256 {
        violations.push("input digest does not match the manifest".to_owned());
    }
    if manifest.records != rebuilt.records {
        violations.push(format!(
            "record count differs: manifest={}, observed={}",
            manifest.records, rebuilt.records
        ));
    }
    if manifest.groups != rebuilt.groups {
        violations.push(format!(
            "group count differs: manifest={}, observed={}",
            manifest.groups, rebuilt.groups
        ));
    }
    if manifest.assignments != rebuilt.assignments {
        violations.push("group assignments do not match the deterministic plan".to_owned());
    }
    if manifest.fold_summaries != rebuilt.fold_summaries {
        violations.push("fold summaries do not match the input and assignments".to_owned());
    }

    Ok(AuditReport {
        valid: violations.is_empty(),
        input_sha256: rebuilt.input_sha256,
        records: rebuilt.records,
        groups: rebuilt.groups,
        violations,
    })
}

fn parse_records(input: &[u8], config: &Config) -> Result<Vec<ParsedRecord>, HoldoutError> {
    config.validate()?;
    let text = std::str::from_utf8(input).map_err(|error| {
        HoldoutError::InvalidConfig(format!("input must be UTF-8 JSONL: {error}"))
    })?;
    let mut records = Vec::new();

    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        if raw.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(raw)
            .map_err(|source| HoldoutError::InvalidJson { line, source })?;
        let group_value =
            get_path(&value, &config.group_field).ok_or_else(|| HoldoutError::MissingField {
                line,
                field: config.group_field.clone(),
            })?;
        let group = scalar_key(group_value).ok_or_else(|| HoldoutError::InvalidScalar {
            line,
            field: config.group_field.clone(),
        })?;
        let label = config
            .label_field
            .as_ref()
            .map(|field| {
                let value = get_path(&value, field).ok_or_else(|| HoldoutError::MissingField {
                    line,
                    field: field.clone(),
                })?;
                scalar_key(value).ok_or_else(|| HoldoutError::InvalidScalar {
                    line,
                    field: field.clone(),
                })
            })
            .transpose()?;

        records.push(ParsedRecord {
            raw: raw.to_owned(),
            group,
            label,
        });
    }

    if records.is_empty() {
        return Err(HoldoutError::EmptyInput);
    }
    Ok(records)
}

fn get_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .try_fold(value, |current, segment| current.as_object()?.get(segment))
}

fn scalar_key(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(format!("string:{value}")),
        Value::Number(value) => Some(format!("number:{value}")),
        Value::Bool(value) => Some(format!("bool:{value}")),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

fn build_plan(input: &[u8], records: &[ParsedRecord], config: Config) -> Manifest {
    let mut grouped: HashMap<String, GroupBucket> = HashMap::new();
    let mut label_totals = BTreeMap::<String, usize>::new();

    for (index, record) in records.iter().enumerate() {
        let bucket = grouped
            .entry(record.group.clone())
            .or_insert_with(|| GroupBucket {
                key: record.group.clone(),
                records: Vec::new(),
                labels: BTreeMap::new(),
                order_hash: seeded_hash(&config.seed, &record.group),
            });
        bucket.records.push(index);
        if let Some(label) = &record.label {
            *bucket.labels.entry(label.clone()).or_default() += 1;
            *label_totals.entry(label.clone()).or_default() += 1;
        }
    }

    let mut groups: Vec<GroupBucket> = grouped.into_values().collect();
    groups.sort_by(|left, right| {
        right
            .records
            .len()
            .cmp(&left.records.len())
            .then_with(|| left.order_hash.cmp(&right.order_hash))
            .then_with(|| left.key.cmp(&right.key))
    });

    let mut folds: Vec<MutableFold> = (0..config.folds).map(|_| MutableFold::default()).collect();
    let mut assignments = BTreeMap::new();

    for group in groups {
        let selected = (0..config.folds)
            .min_by(|left, right| {
                let left_score =
                    placement_score(&folds[*left], &group, records.len(), &label_totals);
                let right_score =
                    placement_score(&folds[*right], &group, records.len(), &label_totals);
                left_score
                    .cmp(&right_score)
                    .then_with(|| folds[*left].records.cmp(&folds[*right].records))
                    .then_with(|| folds[*left].groups.cmp(&folds[*right].groups))
                    .then_with(|| left.cmp(right))
            })
            .expect("config validation guarantees at least two folds");

        let fold = &mut folds[selected];
        fold.records += group.records.len();
        fold.groups += 1;
        for (label, count) in group.labels {
            *fold.labels.entry(label).or_default() += count;
        }
        assignments.insert(group.key, selected);
    }

    let fold_summaries = folds
        .into_iter()
        .enumerate()
        .map(|(index, fold)| FoldSummary {
            index,
            records: fold.records,
            groups: fold.groups,
            labels: fold.labels,
        })
        .collect();

    Manifest {
        schema_version: 1,
        tool_version: env!("CARGO_PKG_VERSION").to_owned(),
        input_sha256: hex_digest(input),
        config,
        records: records.len(),
        groups: assignments.len(),
        fold_summaries,
        assignments,
    }
}

fn placement_score(
    fold: &MutableFold,
    group: &GroupBucket,
    record_total: usize,
    label_totals: &BTreeMap<String, usize>,
) -> u128 {
    let mut score = normalized_square_delta(fold.records, group.records.len(), record_total);
    for (label, added) in &group.labels {
        score += normalized_square_delta(
            *fold.labels.get(label).unwrap_or(&0),
            *added,
            label_totals[label],
        );
    }
    score
}

fn normalized_square_delta(current: usize, added: usize, total: usize) -> u128 {
    let current = current as u128;
    let added = added as u128;
    let total = total as u128;
    let delta = 2 * current * added + added * added;
    delta * SCORE_SCALE / (total * total)
}

fn seeded_hash(seed: &str, key: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    hasher.update([0]);
    hasher.update(key.as_bytes());
    hasher.finalize().into()
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            group_field: "subject.id".to_owned(),
            label_field: Some("outcome".to_owned()),
            folds: 3,
            seed: "trial-7".to_owned(),
        }
    }

    fn sample() -> Vec<u8> {
        (0..18)
            .map(|index| {
                format!(
                    "{{\"subject\":{{\"id\":\"s{}\"}},\"outcome\":\"{}\",\"value\":{index}}}\n",
                    index / 2,
                    if index % 4 < 2 {
                        "positive"
                    } else {
                        "negative"
                    }
                )
            })
            .collect::<String>()
            .into_bytes()
    }

    #[test]
    fn plan_is_deterministic() {
        let first = plan(&sample(), config()).unwrap();
        let second = plan(&sample(), config()).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn every_group_has_one_assignment() {
        let manifest = plan(&sample(), config()).unwrap();
        assert_eq!(manifest.groups, 9);
        assert_eq!(manifest.assignments.len(), 9);
        assert!(manifest.assignments.values().all(|fold| *fold < 3));
    }

    #[test]
    fn split_preserves_every_record() {
        let (manifest, folds) = split(&sample(), config()).unwrap();
        let observed = folds
            .iter()
            .map(|fold| String::from_utf8_lossy(fold).lines().count())
            .sum::<usize>();
        assert_eq!(observed, manifest.records);
        assert_eq!(observed, 18);
    }

    #[test]
    fn groups_do_not_cross_fold_files() {
        let (manifest, folds) = split(&sample(), config()).unwrap();
        for (group, assigned_fold) in &manifest.assignments {
            let raw_group = group.strip_prefix("string:").unwrap();
            for (index, fold) in folds.iter().enumerate() {
                let contains = String::from_utf8_lossy(fold)
                    .lines()
                    .any(|line| line.contains(&format!("\"id\":\"{raw_group}\"")));
                assert_eq!(contains, index == *assigned_fold);
            }
        }
    }

    #[test]
    fn label_distribution_is_balanced_for_regular_groups() {
        let manifest = plan(&sample(), config()).unwrap();
        for label in ["string:positive", "string:negative"] {
            let counts: Vec<usize> = manifest
                .fold_summaries
                .iter()
                .map(|fold| *fold.labels.get(label).unwrap_or(&0))
                .collect();
            assert!(counts.iter().max().unwrap() - counts.iter().min().unwrap() <= 2);
        }
    }

    #[test]
    fn changing_seed_changes_a_nontrivial_plan() {
        let first = plan(&sample(), config()).unwrap();
        let mut changed = config();
        changed.seed = "another-seed".to_owned();
        let second = plan(&sample(), changed).unwrap();
        assert_ne!(first.assignments, second.assignments);
    }

    #[test]
    fn audit_accepts_exact_input() {
        let input = sample();
        let manifest = plan(&input, config()).unwrap();
        let report = audit(&input, &manifest).unwrap();
        assert!(report.valid);
        assert!(report.violations.is_empty());
    }

    #[test]
    fn audit_rejects_changed_input() {
        let input = sample();
        let manifest = plan(&input, config()).unwrap();
        let mut changed = input.clone();
        changed.extend_from_slice(b"{\"subject\":{\"id\":\"late\"},\"outcome\":\"negative\"}\n");
        let report = audit(&changed, &manifest).unwrap();
        assert!(!report.valid);
        assert!(report.violations.iter().any(|item| item.contains("digest")));
    }

    #[test]
    fn audit_rejects_tampered_assignment() {
        let input = sample();
        let mut manifest = plan(&input, config()).unwrap();
        let first = manifest.assignments.values_mut().next().unwrap();
        *first = (*first + 1) % manifest.config.folds;
        let report = audit(&input, &manifest).unwrap();
        assert!(!report.valid);
        assert!(
            report
                .violations
                .iter()
                .any(|item| item.contains("assignments"))
        );
    }

    #[test]
    fn rejects_missing_group_field() {
        let error = plan(b"{\"outcome\":\"ok\"}\n", config()).unwrap_err();
        assert!(matches!(error, HoldoutError::MissingField { .. }));
    }

    #[test]
    fn rejects_composite_group_key() {
        let error = plan(
            b"{\"subject\":{\"id\":[1,2]},\"outcome\":\"ok\"}\n",
            config(),
        )
        .unwrap_err();
        assert!(matches!(error, HoldoutError::InvalidScalar { .. }));
    }

    #[test]
    fn rejects_invalid_fold_count() {
        let mut invalid = config();
        invalid.folds = 1;
        assert!(matches!(
            plan(&sample(), invalid).unwrap_err(),
            HoldoutError::InvalidConfig(_)
        ));
    }

    #[test]
    fn accepts_numeric_group_and_boolean_label() {
        let config = Config {
            group_field: "entity".to_owned(),
            label_field: Some("target".to_owned()),
            folds: 2,
            seed: "typed".to_owned(),
        };
        let manifest = plan(
            b"{\"entity\":1,\"target\":true}\n{\"entity\":2,\"target\":false}\n",
            config,
        )
        .unwrap();
        assert!(manifest.assignments.contains_key("number:1"));
        assert!(
            manifest
                .fold_summaries
                .iter()
                .any(|fold| fold.labels.keys().any(|label| label.starts_with("bool:")))
        );
    }
}
