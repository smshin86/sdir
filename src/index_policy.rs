//! Profile-scoped NTFS indexing policy and validated search-result roots.
//!
//! Include/exclude roots limit search results. They do not shrink the volume
//! relationship graph needed to resolve moved folders and hard links.
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use std::fmt;

pub const MAX_VOLUME_POLICIES: usize = 32;
pub const MAX_ROOTS_PER_POLICY: usize = 32;
pub const MAX_ROOT_UTF16_UNITS: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootValidationError {
    Empty,
    Absolute,
    EmptyComponent,
    CurrentDirectory,
    ParentDirectory,
    DevicePath,
    AlternateDataStream,
    Wildcard,
    ControlCharacter,
    TrailingDotOrSpace,
    TooLong,
}

impl fmt::Display for RootValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Empty => "scope root is empty",
            Self::Absolute => "scope root must be relative to its volume",
            Self::EmptyComponent => "scope root contains an empty path component",
            Self::CurrentDirectory => "scope root contains a current-directory component",
            Self::ParentDirectory => "scope root contains a parent-directory component",
            Self::DevicePath => "scope root contains a device path or device name",
            Self::AlternateDataStream => "scope root contains an alternate-data-stream separator",
            Self::Wildcard => "scope root contains a wildcard",
            Self::ControlCharacter => "scope root contains a control character",
            Self::TrailingDotOrSpace => "scope root component ends with a dot or space",
            Self::TooLong => "scope root exceeds the UTF-16 length limit",
        };
        f.write_str(message)
    }
}

impl std::error::Error for RootValidationError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexPolicyError {
    InvalidGuid,
    Root(RootValidationError),
    TooManyRoots,
    TooManyVolumePolicies,
    InvalidConfiguration,
}

impl fmt::Display for IndexPolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGuid => f.write_str("volume identity is not a canonical GUID"),
            Self::Root(error) => error.fmt(f),
            Self::TooManyRoots => f.write_str("a volume policy may contain at most 32 roots"),
            Self::TooManyVolumePolicies => {
                f.write_str("index settings may contain at most 32 volume policies")
            }
            Self::InvalidConfiguration => {
                f.write_str("invalid index settings are preserved and fail closed")
            }
        }
    }
}

impl std::error::Error for IndexPolicyError {}

/// A canonical UUID value. Drive letters and mount paths are never identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VolumeGuid(String);

impl VolumeGuid {
    pub fn parse(value: &str) -> Result<Self, IndexPolicyError> {
        normalize_guid(value)
            .map(Self)
            .ok_or(IndexPolicyError::InvalidGuid)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn normalize_guid(value: &str) -> Option<String> {
    const VOLUME_PREFIX: &str = r"\\?\Volume{";

    let uuid = if value
        .get(..VOLUME_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(VOLUME_PREFIX))
    {
        value[VOLUME_PREFIX.len()..]
            .strip_suffix('\\')?
            .strip_suffix('}')?
    } else if value.starts_with('{') || value.ends_with('}') {
        value.strip_prefix('{')?.strip_suffix('}')?
    } else {
        value
    };

    if uuid.len() != 36 {
        return None;
    }
    let bytes = uuid.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        let separator = matches!(index, 8 | 13 | 18 | 23);
        if separator {
            if *byte != b'-' {
                return None;
            }
        } else if !byte.is_ascii_hexdigit() {
            return None;
        }
    }
    Some(uuid.to_ascii_lowercase())
}

impl Serialize for VolumeGuid {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for VolumeGuid {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveType {
    Fixed,
    Removable,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NtfsVolumeClass {
    Fixed,
    Removable,
    Unsupported,
}

/// Pure classification for the only supported classes: NTFS fixed/removable.
pub fn classify_ntfs_volume(filesystem: &str, drive_type: DriveType) -> NtfsVolumeClass {
    if !filesystem.eq_ignore_ascii_case("NTFS") {
        return NtfsVolumeClass::Unsupported;
    }
    match drive_type {
        DriveType::Fixed => NtfsVolumeClass::Fixed,
        DriveType::Removable => NtfsVolumeClass::Removable,
        DriveType::Other => NtfsVolumeClass::Unsupported,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeOverride {
    #[default]
    Auto,
    Include,
    Exclude,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OfflinePolicy {
    /// Release live state and remove this profile's permitted cache file.
    #[default]
    DropCache,
    /// Release live state but retain this profile's permitted disk cache.
    KeepCache,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfflineCacheAction {
    DeleteOwnedCache,
    RetainOwnedCache,
}

impl OfflinePolicy {
    pub fn cache_action(self) -> OfflineCacheAction {
        match self {
            Self::DropCache => OfflineCacheAction::DeleteOwnedCache,
            Self::KeepCache => OfflineCacheAction::RetainOwnedCache,
        }
    }
}

/// A validated path relative to a volume root, normalized to forward slashes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct RelativePathRoot(String);

impl RelativePathRoot {
    pub fn parse(value: &str) -> Result<Self, RootValidationError> {
        if value.is_empty() {
            return Err(RootValidationError::Empty);
        }
        if value.encode_utf16().count() > MAX_ROOT_UTF16_UNITS {
            return Err(RootValidationError::TooLong);
        }
        if value.starts_with('/') || value.starts_with('\\') {
            return Err(RootValidationError::Absolute);
        }
        if value.chars().any(char::is_control) {
            return Err(RootValidationError::ControlCharacter);
        }

        let separated = value.replace('\\', "/");
        let normalized = separated.trim_end_matches('/');
        if normalized.is_empty() {
            return Err(RootValidationError::Empty);
        }

        let mut components = Vec::new();
        for component in normalized.split('/') {
            if component.is_empty() {
                return Err(RootValidationError::EmptyComponent);
            }
            validate_component(component)?;
            components.push(component);
        }
        let normalized = components.join("/");
        if normalized.encode_utf16().count() > MAX_ROOT_UTF16_UNITS {
            return Err(RootValidationError::TooLong);
        }
        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Compares complete path components. Callers may supply the same
    /// case-folding rule used by the query engine.
    pub fn matches_path_by<F>(&self, relative_path: &str, mut equal_component: F) -> bool
    where
        F: FnMut(&str, &str) -> bool,
    {
        let Some(path_components) = relative_components(relative_path) else {
            return false;
        };
        let root_components = self.0.split('/').collect::<Vec<_>>();
        path_components.len() >= root_components.len()
            && root_components
                .iter()
                .zip(path_components.iter())
                .all(|(root, path)| equal_component(root, path))
    }

    pub fn matches_path(&self, relative_path: &str) -> bool {
        self.matches_path_by(relative_path, |left, right| left == right)
    }

    fn duplicate_key(&self) -> String {
        self.0.to_lowercase()
    }
}

fn validate_component(component: &str) -> Result<(), RootValidationError> {
    if component == "." {
        return Err(RootValidationError::CurrentDirectory);
    }
    if component == ".." {
        return Err(RootValidationError::ParentDirectory);
    }
    if component.contains(':') {
        return Err(RootValidationError::AlternateDataStream);
    }
    if component.contains('*') || component.contains('?') {
        return Err(RootValidationError::Wildcard);
    }
    if component.chars().any(char::is_control) {
        return Err(RootValidationError::ControlCharacter);
    }
    if component.ends_with('.') || component.ends_with(' ') {
        return Err(RootValidationError::TrailingDotOrSpace);
    }
    if is_reserved_device_name(component) {
        return Err(RootValidationError::DevicePath);
    }
    Ok(())
}

fn is_reserved_device_name(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or(component);
    let upper = stem.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    let bytes = upper.as_bytes();
    bytes.len() == 4
        && (&bytes[..3] == b"COM" || &bytes[..3] == b"LPT")
        && matches!(bytes[3], b'1'..=b'9')
}

fn relative_components(path: &str) -> Option<Vec<&str>> {
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path
            .chars()
            .any(|character| character.is_control() || matches!(character, ':' | '*' | '?'))
    {
        return None;
    }
    let normalized = path.trim_end_matches(|character| character == '/' || character == '\\');
    if normalized.is_empty() {
        return None;
    }
    let components = normalized
        .split(|character| character == '/' || character == '\\')
        .collect::<Vec<_>>();
    if components
        .iter()
        .any(|component| component.is_empty() || *component == "." || *component == "..")
    {
        return None;
    }
    Some(components)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ScopePolicy {
    include_roots: Vec<RelativePathRoot>,
    exclude_roots: Vec<RelativePathRoot>,
}

impl ScopePolicy {
    pub fn new(
        include_roots: Vec<RelativePathRoot>,
        exclude_roots: Vec<RelativePathRoot>,
    ) -> Result<Self, IndexPolicyError> {
        if include_roots.len().saturating_add(exclude_roots.len()) > MAX_ROOTS_PER_POLICY {
            return Err(IndexPolicyError::TooManyRoots);
        }
        Ok(Self {
            include_roots: normalize_roots(include_roots),
            exclude_roots: normalize_roots(exclude_roots),
        })
    }

    pub fn include_roots(&self) -> &[RelativePathRoot] {
        &self.include_roots
    }

    pub fn exclude_roots(&self) -> &[RelativePathRoot] {
        &self.exclude_roots
    }

    /// Empty includes mean all valid paths; excludes always take precedence.
    pub fn allows_path_by<F>(&self, relative_path: &str, mut equal_component: F) -> bool
    where
        F: FnMut(&str, &str) -> bool,
    {
        if relative_components(relative_path).is_none() {
            return false;
        }
        if self
            .exclude_roots
            .iter()
            .any(|root| root.matches_path_by(relative_path, &mut equal_component))
        {
            return false;
        }
        self.include_roots.is_empty()
            || self
                .include_roots
                .iter()
                .any(|root| root.matches_path_by(relative_path, &mut equal_component))
    }

    pub fn allows_path(&self, relative_path: &str) -> bool {
        self.allows_path_by(relative_path, |left, right| left == right)
    }
}

fn normalize_roots(mut roots: Vec<RelativePathRoot>) -> Vec<RelativePathRoot> {
    roots.sort_by(|left, right| {
        left.duplicate_key()
            .cmp(&right.duplicate_key())
            .then_with(|| left.0.cmp(&right.0))
    });
    roots.dedup_by(|left, right| left.duplicate_key() == right.duplicate_key());
    roots
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct VolumePolicy {
    guid: VolumeGuid,
    mode: VolumeOverride,
    monitor: bool,
    #[serde(flatten)]
    scope: ScopePolicy,
}

impl VolumePolicy {
    pub fn new(guid: VolumeGuid, mode: VolumeOverride, monitor: bool, scope: ScopePolicy) -> Self {
        Self {
            guid,
            mode,
            monitor,
            scope,
        }
    }

    pub fn guid(&self) -> &VolumeGuid {
        &self.guid
    }

    pub fn mode(&self) -> VolumeOverride {
        self.mode
    }

    pub fn monitor(&self) -> bool {
        self.monitor
    }

    pub fn scope(&self) -> &ScopePolicy {
        &self.scope
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeDecision {
    Included { monitor: bool },
    Excluded,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NtfsIndexingSettings {
    auto_fixed: bool,
    auto_removable: bool,
    offline: OfflinePolicy,
    persistent: bool,
    volumes: Vec<VolumePolicy>,
    valid: bool,
    invalid_raw: Option<Value>,
}

impl Default for NtfsIndexingSettings {
    fn default() -> Self {
        Self {
            auto_fixed: true,
            auto_removable: false,
            offline: OfflinePolicy::DropCache,
            persistent: false,
            volumes: Vec::new(),
            valid: true,
            invalid_raw: None,
        }
    }
}

impl NtfsIndexingSettings {
    pub fn auto_fixed(&self) -> bool {
        self.auto_fixed
    }

    pub fn auto_removable(&self) -> bool {
        self.auto_removable
    }

    pub fn offline_policy(&self) -> OfflinePolicy {
        self.offline
    }

    pub fn offline_cache_action(&self) -> OfflineCacheAction {
        self.offline.cache_action()
    }

    pub fn persistent(&self) -> bool {
        self.persistent
    }

    pub fn is_valid(&self) -> bool {
        self.valid
    }

    pub fn volume_policies(&self) -> &[VolumePolicy] {
        &self.volumes
    }

    pub fn volume_policy(&self, guid: &VolumeGuid) -> Option<&VolumePolicy> {
        self.volumes.iter().find(|policy| &policy.guid == guid)
    }

    pub fn decision_for(&self, guid: &VolumeGuid, class: NtfsVolumeClass) -> VolumeDecision {
        if class == NtfsVolumeClass::Unsupported {
            return VolumeDecision::Unsupported;
        }
        if !self.valid {
            return VolumeDecision::Excluded;
        }

        let policy = self.volume_policy(guid);
        let mode = policy.map_or(VolumeOverride::Auto, VolumePolicy::mode);
        let included = match mode {
            VolumeOverride::Include => true,
            VolumeOverride::Exclude => false,
            VolumeOverride::Auto => match class {
                NtfsVolumeClass::Fixed => self.auto_fixed,
                NtfsVolumeClass::Removable => self.auto_removable,
                NtfsVolumeClass::Unsupported => false,
            },
        };
        if included {
            VolumeDecision::Included {
                monitor: policy.map_or(true, VolumePolicy::monitor),
            }
        } else {
            VolumeDecision::Excluded
        }
    }

    pub fn scope_allows_path_by<F>(
        &self,
        guid: &VolumeGuid,
        relative_path: &str,
        equal_component: F,
    ) -> bool
    where
        F: FnMut(&str, &str) -> bool,
    {
        if !self.valid {
            return false;
        }
        self.volume_policy(guid).map_or(true, |policy| {
            policy.scope.allows_path_by(relative_path, equal_component)
        })
    }

    pub fn scope_allows_path(&self, guid: &VolumeGuid, relative_path: &str) -> bool {
        self.scope_allows_path_by(guid, relative_path, |left, right| left == right)
    }

    pub fn set_auto_fixed(&mut self, value: bool) -> Result<(), IndexPolicyError> {
        self.ensure_valid()?;
        self.auto_fixed = value;
        Ok(())
    }

    pub fn set_auto_removable(&mut self, value: bool) -> Result<(), IndexPolicyError> {
        self.ensure_valid()?;
        self.auto_removable = value;
        Ok(())
    }

    pub fn set_offline_policy(&mut self, value: OfflinePolicy) -> Result<(), IndexPolicyError> {
        self.ensure_valid()?;
        self.offline = value;
        Ok(())
    }

    pub fn set_persistent(&mut self, value: bool) -> Result<(), IndexPolicyError> {
        self.ensure_valid()?;
        self.persistent = value;
        Ok(())
    }

    pub fn set_volume_policy(&mut self, policy: VolumePolicy) -> Result<(), IndexPolicyError> {
        self.ensure_valid()?;
        if let Some(index) = self
            .volumes
            .iter()
            .position(|current| current.guid == policy.guid)
        {
            self.volumes[index] = policy;
        } else {
            if self.volumes.len() >= MAX_VOLUME_POLICIES {
                return Err(IndexPolicyError::TooManyVolumePolicies);
            }
            self.volumes.push(policy);
        }
        self.volumes
            .sort_by(|left, right| left.guid.cmp(&right.guid));
        Ok(())
    }

    pub fn remove_volume_policy(&mut self, guid: &VolumeGuid) -> Result<bool, IndexPolicyError> {
        self.ensure_valid()?;
        let before = self.volumes.len();
        self.volumes.retain(|policy| &policy.guid != guid);
        Ok(self.volumes.len() != before)
    }

    /// Explicit repair/reset for a future user initiated settings action.
    pub fn reset_to_defaults(&mut self) {
        *self = Self::default();
    }

    fn ensure_valid(&self) -> Result<(), IndexPolicyError> {
        if self.valid {
            Ok(())
        } else {
            Err(IndexPolicyError::InvalidConfiguration)
        }
    }

    fn from_value(value: Value) -> Self {
        match parse_valid_settings(&value) {
            Ok(settings) => settings,
            Err(_) => {
                let (auto_fixed, auto_removable, offline, persistent) = safe_values(&value);
                Self {
                    auto_fixed,
                    auto_removable,
                    offline,
                    persistent,
                    volumes: Vec::new(),
                    valid: false,
                    // Keep malformed and unknown policy data intact through profile saves.
                    invalid_raw: Some(value),
                }
            }
        }
    }
}

#[derive(Serialize)]
struct IndexingSettingsWire<'a> {
    auto_fixed: bool,
    auto_removable: bool,
    offline: OfflinePolicy,
    persistent: bool,
    volumes: &'a [VolumePolicy],
}

impl Serialize for NtfsIndexingSettings {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if let Some(raw) = &self.invalid_raw {
            return raw.serialize(serializer);
        }
        IndexingSettingsWire {
            auto_fixed: self.auto_fixed,
            auto_removable: self.auto_removable,
            offline: self.offline,
            persistent: self.persistent,
            volumes: &self.volumes,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for NtfsIndexingSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self::from_value(Value::deserialize(deserializer)?))
    }
}

fn parse_valid_settings(value: &Value) -> Result<NtfsIndexingSettings, IndexPolicyError> {
    let object = value
        .as_object()
        .ok_or(IndexPolicyError::InvalidConfiguration)?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "auto_fixed" | "auto_removable" | "offline" | "persistent" | "volumes"
        )
    }) {
        return Err(IndexPolicyError::InvalidConfiguration);
    }

    let auto_fixed = read_bool(object, "auto_fixed", true)?;
    let auto_removable = read_bool(object, "auto_removable", false)?;
    let persistent = read_bool(object, "persistent", false)?;
    let offline = match object.get("offline") {
        None => OfflinePolicy::DropCache,
        Some(Value::String(value)) if value == "drop_cache" => OfflinePolicy::DropCache,
        Some(Value::String(value)) if value == "keep_cache" => OfflinePolicy::KeepCache,
        _ => return Err(IndexPolicyError::InvalidConfiguration),
    };

    let raw_volumes = match object.get("volumes") {
        None => &[][..],
        Some(Value::Array(volumes)) if volumes.len() <= MAX_VOLUME_POLICIES => volumes.as_slice(),
        _ => return Err(IndexPolicyError::TooManyVolumePolicies),
    };
    let mut volumes = raw_volumes
        .iter()
        .map(parse_volume_policy)
        .collect::<Result<Vec<_>, _>>()?;
    volumes.sort_by(|left, right| left.guid.cmp(&right.guid));

    let mut normalized = Vec::with_capacity(volumes.len());
    for policy in volumes {
        if let Some(previous) = normalized.last() {
            let previous: &VolumePolicy = previous;
            if previous.guid == policy.guid {
                if previous == &policy {
                    continue;
                }
                return Err(IndexPolicyError::InvalidConfiguration);
            }
        }
        normalized.push(policy);
    }
    if normalized.len() > MAX_VOLUME_POLICIES {
        return Err(IndexPolicyError::TooManyVolumePolicies);
    }

    Ok(NtfsIndexingSettings {
        auto_fixed,
        auto_removable,
        offline,
        persistent,
        volumes: normalized,
        valid: true,
        invalid_raw: None,
    })
}

fn safe_values(value: &Value) -> (bool, bool, OfflinePolicy, bool) {
    let Some(object) = value.as_object() else {
        return (false, false, OfflinePolicy::DropCache, false);
    };
    let safe_bool = |field: &str, default: bool| match object.get(field) {
        None => default,
        Some(Value::Bool(value)) => *value,
        _ => false,
    };
    let offline = match object.get("offline") {
        None => OfflinePolicy::DropCache,
        Some(Value::String(value)) if value == "drop_cache" => OfflinePolicy::DropCache,
        Some(Value::String(value)) if value == "keep_cache" => OfflinePolicy::KeepCache,
        _ => OfflinePolicy::DropCache,
    };
    (
        safe_bool("auto_fixed", true),
        safe_bool("auto_removable", false),
        offline,
        safe_bool("persistent", false),
    )
}

fn read_bool(
    object: &Map<String, Value>,
    field: &str,
    default: bool,
) -> Result<bool, IndexPolicyError> {
    match object.get(field) {
        None => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        _ => Err(IndexPolicyError::InvalidConfiguration),
    }
}

fn parse_volume_policy(value: &Value) -> Result<VolumePolicy, IndexPolicyError> {
    let object = value
        .as_object()
        .ok_or(IndexPolicyError::InvalidConfiguration)?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "guid" | "mode" | "monitor" | "include_roots" | "exclude_roots"
        )
    }) {
        return Err(IndexPolicyError::InvalidConfiguration);
    }

    let guid = object
        .get("guid")
        .and_then(Value::as_str)
        .ok_or(IndexPolicyError::InvalidConfiguration)
        .and_then(VolumeGuid::parse)?;
    let mode = match object.get("mode") {
        None => VolumeOverride::Auto,
        Some(Value::String(value)) if value == "auto" => VolumeOverride::Auto,
        Some(Value::String(value)) if value == "include" => VolumeOverride::Include,
        Some(Value::String(value)) if value == "exclude" => VolumeOverride::Exclude,
        _ => return Err(IndexPolicyError::InvalidConfiguration),
    };
    let monitor = read_bool(object, "monitor", true)?;
    let include_roots = read_roots(object, "include_roots")?;
    let exclude_roots = read_roots(object, "exclude_roots")?;
    let scope = ScopePolicy::new(include_roots, exclude_roots)?;
    Ok(VolumePolicy::new(guid, mode, monitor, scope))
}

fn read_roots(
    object: &Map<String, Value>,
    field: &str,
) -> Result<Vec<RelativePathRoot>, IndexPolicyError> {
    match object.get(field) {
        None => Ok(Vec::new()),
        Some(Value::Array(roots)) if roots.len() <= MAX_ROOTS_PER_POLICY => roots
            .iter()
            .map(|root| {
                root.as_str()
                    .ok_or(IndexPolicyError::InvalidConfiguration)
                    .and_then(|root| RelativePathRoot::parse(root).map_err(IndexPolicyError::Root))
            })
            .collect(),
        Some(Value::Array(_)) => Err(IndexPolicyError::TooManyRoots),
        _ => Err(IndexPolicyError::InvalidConfiguration),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guid(number: u32) -> VolumeGuid {
        VolumeGuid::parse(&format!("00000000-0000-0000-0000-{number:012x}")).unwrap()
    }

    fn root(value: &str) -> RelativePathRoot {
        RelativePathRoot::parse(value).unwrap()
    }

    fn volume_policy(
        guid: VolumeGuid,
        mode: VolumeOverride,
        monitor: bool,
        scope: ScopePolicy,
    ) -> VolumePolicy {
        VolumePolicy::new(guid, mode, monitor, scope)
    }

    #[test]
    fn defaults_and_supported_ntfs_classification_are_explicit() {
        let settings = NtfsIndexingSettings::default();
        assert!(settings.auto_fixed());
        assert!(!settings.auto_removable());
        assert_eq!(settings.offline_policy(), OfflinePolicy::DropCache);
        assert!(!settings.persistent());
        assert!(settings.is_valid());

        assert_eq!(
            classify_ntfs_volume("ntfs", DriveType::Fixed),
            NtfsVolumeClass::Fixed
        );
        assert_eq!(
            classify_ntfs_volume("NTFS", DriveType::Removable),
            NtfsVolumeClass::Removable
        );
        assert_eq!(
            classify_ntfs_volume("ReFS", DriveType::Fixed),
            NtfsVolumeClass::Unsupported
        );
        assert_eq!(
            classify_ntfs_volume("NTFS", DriveType::Other),
            NtfsVolumeClass::Unsupported
        );

        assert_eq!(
            settings.decision_for(&guid(1), NtfsVolumeClass::Fixed),
            VolumeDecision::Included { monitor: true }
        );
        assert_eq!(
            settings.decision_for(&guid(2), NtfsVolumeClass::Removable),
            VolumeDecision::Excluded
        );
    }

    #[test]
    fn manual_include_exclude_and_auto_precedence_is_per_guid() {
        let mut settings = NtfsIndexingSettings::default();
        let removable = guid(1);
        let fixed = guid(2);
        let auto = guid(3);
        let empty_scope = ScopePolicy::default();

        settings
            .set_volume_policy(volume_policy(
                removable.clone(),
                VolumeOverride::Include,
                false,
                empty_scope.clone(),
            ))
            .unwrap();
        settings
            .set_volume_policy(volume_policy(
                fixed.clone(),
                VolumeOverride::Exclude,
                true,
                empty_scope.clone(),
            ))
            .unwrap();
        settings
            .set_volume_policy(volume_policy(
                auto.clone(),
                VolumeOverride::Auto,
                true,
                empty_scope,
            ))
            .unwrap();

        assert_eq!(
            settings.decision_for(&removable, NtfsVolumeClass::Removable),
            VolumeDecision::Included { monitor: false }
        );
        assert_eq!(
            settings.decision_for(&fixed, NtfsVolumeClass::Fixed),
            VolumeDecision::Excluded
        );
        assert_eq!(
            settings.decision_for(&auto, NtfsVolumeClass::Fixed),
            VolumeDecision::Included { monitor: true }
        );
        assert_eq!(
            settings.decision_for(&removable, NtfsVolumeClass::Unsupported),
            VolumeDecision::Unsupported
        );
    }

    #[test]
    fn guid_identity_survives_mount_changes_without_inheriting_reused_letters() {
        let mut settings = NtfsIndexingSettings::default();
        let original = VolumeGuid::parse("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").unwrap();
        let volume_path = format!("{}{}{}{}", "\\\\?\\Volume{", original.as_str(), "}", "\\");
        let from_mount_path = VolumeGuid::parse(&volume_path).unwrap();
        let from_uppercase = VolumeGuid::parse("AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE").unwrap();
        assert_eq!(original, from_mount_path);
        assert_eq!(original, from_uppercase);

        settings
            .set_volume_policy(volume_policy(
                original.clone(),
                VolumeOverride::Exclude,
                true,
                ScopePolicy::default(),
            ))
            .unwrap();

        let former_mount = "E:";
        let new_mount = "Z:";
        let reused_mount = "E:";
        assert_ne!(former_mount, new_mount);
        assert_eq!(
            settings.decision_for(&original, NtfsVolumeClass::Fixed),
            settings.decision_for(&from_mount_path, NtfsVolumeClass::Fixed)
        );
        assert_eq!(former_mount, reused_mount);
        assert_ne!(new_mount, reused_mount);
        assert_eq!(
            settings.decision_for(&guid(9), NtfsVolumeClass::Fixed),
            VolumeDecision::Included { monitor: true }
        );
    }

    #[test]
    fn offline_policy_controls_disk_cache_while_live_state_is_released() {
        let mut settings = NtfsIndexingSettings::default();
        assert_eq!(
            settings.offline_cache_action(),
            OfflineCacheAction::DeleteOwnedCache
        );
        settings
            .set_offline_policy(OfflinePolicy::KeepCache)
            .unwrap();
        assert_eq!(
            settings.offline_cache_action(),
            OfflineCacheAction::RetainOwnedCache
        );
    }

    #[test]
    fn scope_checks_components_unicode_and_exclude_precedence() {
        let scope =
            ScopePolicy::new(vec![root("자료/日本語")], vec![root("자료/日本語/비공개")]).unwrap();
        assert_eq!(scope.include_roots()[0].as_str(), "자료/日本語");
        assert!(scope.allows_path("자료\\日本語\\문서.txt"));
        assert!(!scope.allows_path("자료/日本語-old/문서.txt"));
        assert!(!scope.allows_path("자료/日本語/비공개/문서.txt"));
        assert!(!scope.allows_path("../자료/日本語/문서.txt"));

        let folded = root("Shared");
        assert!(folded.matches_path_by("shared/file.txt", |left, right| {
            left.eq_ignore_ascii_case(right)
        }));
    }

    #[test]
    fn roots_reject_ambiguous_absolute_parent_device_ads_wildcard_and_dot_space_paths() {
        let invalid = [
            "",
            "/absolute",
            "C:\\absolute",
            "\\\\server\\share",
            "folder/../outside",
            ".",
            "folder/./child",
            "\\\\?\\C:\\device",
            "folder:stream",
            "folder/*",
            "folder/?.txt",
            "line\nbreak",
            "trailing.",
            "trailing ",
            "folder/CON.txt",
            "folder//child",
        ];
        for value in invalid {
            assert!(
                RelativePathRoot::parse(value).is_err(),
                "unexpectedly accepted {value:?}"
            );
        }
    }

    #[test]
    fn root_and_policy_limits_use_utf16_and_deduplicate_deterministically() {
        let within_limit = format!("{}😀", "a".repeat(510));
        assert_eq!(within_limit.encode_utf16().count(), 512);
        assert!(RelativePathRoot::parse(&within_limit).is_ok());
        let over_limit = format!("{}😀", "a".repeat(511));
        assert_eq!(over_limit.encode_utf16().count(), 513);
        assert_eq!(
            RelativePathRoot::parse(&over_limit),
            Err(RootValidationError::TooLong)
        );

        let normalized =
            ScopePolicy::new(vec![root("A\\B"), root("a/b"), root("A/b")], Vec::new()).unwrap();
        assert_eq!(normalized.include_roots().len(), 1);
        assert_eq!(normalized.include_roots()[0].as_str(), "A/B");

        let too_many = (0..=MAX_ROOTS_PER_POLICY)
            .map(|index| root(&format!("root{index}")))
            .collect::<Vec<_>>();
        assert_eq!(
            ScopePolicy::new(too_many, Vec::new()),
            Err(IndexPolicyError::TooManyRoots)
        );

        let mut settings = NtfsIndexingSettings::default();
        for number in 0..MAX_VOLUME_POLICIES as u32 {
            settings
                .set_volume_policy(volume_policy(
                    guid(number),
                    VolumeOverride::Auto,
                    true,
                    ScopePolicy::default(),
                ))
                .unwrap();
        }
        assert_eq!(settings.volume_policies().len(), MAX_VOLUME_POLICIES);
        assert_eq!(
            settings.set_volume_policy(volume_policy(
                guid(MAX_VOLUME_POLICIES as u32),
                VolumeOverride::Auto,
                true,
                ScopePolicy::default(),
            )),
            Err(IndexPolicyError::TooManyVolumePolicies)
        );
    }

    #[test]
    fn invalid_scope_configuration_round_trips_unchanged_and_blocks_results() {
        let raw = serde_json::json!({
            "auto_fixed": true,
            "auto_removable": false,
            "offline": "keep_cache",
            "persistent": true,
            "volumes": [{
                "guid": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
                "mode": "include",
                "monitor": true,
                "include_roots": ["../outside"],
                "exclude_roots": []
            }]
        });
        let settings: NtfsIndexingSettings = serde_json::from_value(raw.clone()).unwrap();
        let volume = VolumeGuid::parse("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").unwrap();

        assert!(!settings.is_valid());
        assert_eq!(
            settings.decision_for(&volume, NtfsVolumeClass::Fixed),
            VolumeDecision::Excluded
        );
        assert!(!settings.scope_allows_path(&volume, "anything"));
        assert_eq!(settings.offline_policy(), OfflinePolicy::KeepCache);
        assert!(settings.persistent());
        assert_eq!(serde_json::to_value(settings).unwrap(), raw);
    }
}
