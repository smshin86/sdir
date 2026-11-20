use crate::model::NativePath;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenamePair {
    pub source: NativePath,
    pub name: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Pattern {
    pub find: String,
    pub replace: String,
    pub prefix: String,
    pub suffix: String,
    pub start: Option<u32>,
}
pub fn preview(paths: &[NativePath], p: &Pattern) -> Result<Vec<RenamePair>, String> {
    if paths.is_empty() || paths.len() > 500 {
        return Err("이름 일괄 변경은 1~500개 항목을 선택하세요.".into());
    }
    if paths.iter().map(|p| p.0.len()).sum::<usize>() > 50000 {
        return Err("경로 데이터가 많습니다. 항목을 나눠 변경하세요.".into());
    }
    let mut result = vec![];
    for (i, path) in paths.iter().enumerate() {
        let native = path.to_path();
        let directory = native.is_dir();
        let original = path.name();
        let (stem, extension) = if directory {
            (original.as_str(), None)
        } else {
            (
                native
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&original),
                native.extension().and_then(|s| s.to_str()),
            )
        };
        let stem = if p.find.is_empty() {
            stem.to_string()
        } else {
            stem.replace(&p.find, &p.replace)
        };
        let counter = p
            .start
            .map(|n| format!("_{}", u64::from(n) + i as u64))
            .unwrap_or_default();
        let mut name = format!("{}{}{}{}", p.prefix, stem, p.suffix, counter);
        if let Some(ext) = extension {
            if !ext.is_empty() {
                name.push('.');
                name.push_str(ext);
            }
        }
        result.push(RenamePair {
            source: path.clone(),
            name,
        });
    }
    validate(&result)?;
    Ok(result)
}
pub fn validate(pairs: &[RenamePair]) -> Result<(), String> {
    if pairs.is_empty() || pairs.len() > 500 {
        return Err("변경할 항목이 없거나 한도를 초과했습니다.".into());
    }
    let mut targets = std::collections::BTreeSet::new();
    for pair in pairs {
        crate::filesystem::validate_name(&pair.name)?;
        let parent = pair
            .source
            .parent()
            .ok_or("드라이브 루트는 이름을 바꿀 수 없습니다.")?;
        let target = parent.to_path().join(&pair.name);
        if !targets.insert(target.to_string_lossy().to_lowercase()) {
            return Err(format!("중복될 이름: {}", pair.name));
        }
        let original = std::fs::canonicalize(pair.source.to_path()).map_err(|e| e.to_string())?;
        if target.try_exists().map_err(|e| e.to_string())?
            && std::fs::canonicalize(&target).map_err(|e| e.to_string())? != original
        {
            return Err(format!("이미 존재하는 이름: {}", pair.name));
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collision_preview_changes_no_files() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), b"a").unwrap();
        std::fs::write(d.path().join("b.txt"), b"b").unwrap();
        let a = NativePath::new(d.path().join("a.txt"));
        assert!(
            preview(
                &[a.clone()],
                &Pattern {
                    find: "a".into(),
                    replace: "b".into(),
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert_eq!(std::fs::read(a.to_path()).unwrap(), b"a");
        let p = preview(
            &[a],
            &Pattern {
                prefix: "new-".into(),
                start: Some(7),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(p[0].name, "new-a_7.txt");
    }
}
