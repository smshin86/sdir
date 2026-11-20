use crate::model::{FileAction, ItemOutcome, JobResult};
#[derive(Clone)]
pub struct Entry {
    pub action: Option<FileAction>,
    pub label: String,
    pub destination: Option<crate::model::NativePath>,
    pub retried: bool,
    pub time: String,
    pub message: String,
    pub outcomes: Vec<ItemOutcome>,
    pub destinations: Vec<crate::model::NativePath>,
    pub cancelled: bool,
}
impl Entry {
    pub fn new(action: FileAction, result: &JobResult) -> Self {
        Self {
            label: action.label().into(),
            destination: action.destination(),
            retried: false,
            action: {
                let sources = action.sources();
                (sources.len() <= 500
                    && sources.iter().map(|p| p.0.len()).sum::<usize>() <= 50000
                    && !matches!(action, FileAction::History { .. }))
                .then_some(action)
            },
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
            message: result.message.clone(),
            outcomes: {
                let mut units = 0;
                result
                    .outcomes
                    .iter()
                    .take(500)
                    .take_while(|o| {
                        units += o.source.0.len();
                        units <= 50000
                    })
                    .cloned()
                    .collect()
            },
            destinations: {
                let mut units = 0;
                result
                    .destinations
                    .iter()
                    .take(500)
                    .take_while(|p| {
                        units += p.0.len();
                        units <= 50000
                    })
                    .cloned()
                    .collect()
            },
            cancelled: result.cancelled,
        }
    }
    // An absent receipt is unknown, never evidence that replay is safe.
    pub fn retry(&self) -> Option<FileAction> {
        if self.retried {
            return None;
        }
        let failed: Vec<_> = self
            .outcomes
            .iter()
            .filter(|o| !o.ok)
            .map(|o| o.source.clone())
            .collect();
        if failed.is_empty() {
            return None;
        }
        let action = match self.action.as_ref()? {
            FileAction::Copy {
                sources,
                destination,
            } => FileAction::Copy {
                sources: sources
                    .iter()
                    .filter(|p| failed.contains(p))
                    .cloned()
                    .collect(),
                destination: destination.clone(),
            },
            FileAction::Move {
                sources,
                destination,
            } => FileAction::Move {
                sources: sources
                    .iter()
                    .filter(|p| failed.contains(p))
                    .cloned()
                    .collect(),
                destination: destination.clone(),
            },
            FileAction::Recycle { sources } => FileAction::Recycle {
                sources: sources
                    .iter()
                    .filter(|p| failed.contains(p))
                    .cloned()
                    .collect(),
            },
            FileAction::Rename { source, name } if failed.contains(source) => FileAction::Rename {
                source: source.clone(),
                name: name.clone(),
            },
            FileAction::BatchRename { pairs } => FileAction::BatchRename {
                pairs: pairs
                    .iter()
                    .filter(|p| failed.contains(&p.source))
                    .cloned()
                    .collect(),
            },
            _ => return None,
        };
        (!action.sources().is_empty()).then_some(action)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NativePath;
    #[test]
    fn retry_excludes_success_and_unknown() {
        let a = NativePath::new("a");
        let b = NativePath::new("b");
        let c = NativePath::new("c");
        let e = Entry::new(
            FileAction::Copy {
                sources: vec![a.clone(), b.clone(), c],
                destination: NativePath::new("dest"),
            },
            &JobResult {
                ok: false,
                cancelled: false,
                message: "partial".into(),
                destinations: vec![],
                undo: None,
                history_note: None,
                outcomes: vec![
                    ItemOutcome {
                        source: a,
                        ok: true,
                        message: "".into(),
                    },
                    ItemOutcome {
                        source: b.clone(),
                        ok: false,
                        message: "failed".into(),
                    },
                ],
            },
        );
        assert_eq!(e.retry().unwrap().sources(), vec![b]);
    }
}
