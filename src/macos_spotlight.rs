use crate::{
    model::NativePath,
    search::{SearchHit, SearchResult, validate_query_text},
};
use std::{
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Read},
    os::unix::ffi::OsStringExt,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const RESULT_LIMIT: usize = 200;
const QUERY_TIMEOUT: Duration = Duration::from_secs(3);

fn match_expression(text: &str) -> String {
    let extension = text.strip_prefix("ext:");
    let text = extension.map_or(text, |ext| ext.trim_start_matches('.'));
    let escaped: String = text
        .chars()
        .flat_map(|character| {
            let escape = matches!(character, '\\' | '"');
            escape
                .then_some('\\')
                .into_iter()
                .chain(std::iter::once(character))
        })
        .collect();
    let value = if extension.is_some() {
        format!("*.{escaped}")
    } else if text.contains(['*', '?']) {
        escaped
    } else {
        format!("*{escaped}*")
    };
    format!("kMDItemFSName == \"{value}\"cd")
}

pub(crate) fn query(text: &str) -> Result<SearchResult, String> {
    let start = Instant::now();
    let text = text.trim();
    validate_query_text(text)?;
    if text.is_empty() {
        return Ok(SearchResult {
            hits: vec![],
            total: 0,
            elapsed_ms: 0.,
            complete: true,
        });
    }
    if text.chars().any(char::is_control) {
        return Err("파일 검색어에 제어 문자를 사용할 수 없습니다.".into());
    }
    let mut child = Command::new("/usr/bin/mdfind")
        .arg("-0")
        .arg(match_expression(text))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    let stdout = child.stdout.take().expect("piped Spotlight output");
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let reader = std::thread::spawn(move || {
        let result = read_hits(BufReader::new(stdout));
        let _ = send.send(result);
    });
    let result = receive.recv_timeout(QUERY_TIMEOUT);
    let mut finished = false;
    if matches!(&result, Ok(Ok((_, true)))) {
        while !finished && start.elapsed() < QUERY_TIMEOUT {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => finished = true,
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                _ => break,
            }
        }
    }
    if !finished {
        let _ = child.kill();
    }
    let _ = child.wait();
    let _ = reader.join();
    let (hits, complete) =
        result.map_err(|_| "Spotlight 검색 시간이 초과됐습니다.".to_string())??;
    if complete && !finished {
        return Err("Spotlight 검색을 완료하지 못했습니다.".into());
    }
    Ok(SearchResult {
        total: hits.len() as u64,
        hits,
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.,
        complete,
    })
}

fn read_hits(mut input: impl BufRead) -> Result<(Vec<SearchHit>, bool), String> {
    let mut hits = Vec::new();
    let mut line = Vec::new();
    loop {
        let bytes = input
            .by_ref()
            .take(8192)
            .read_until(0, &mut line)
            .map_err(|error| error.to_string())?;
        if bytes == 0 {
            return Ok((hits, true));
        }
        if line.pop() != Some(0) {
            return Err("Spotlight 검색 결과의 경로가 너무 깁니다.".into());
        }
        let path = PathBuf::from(OsString::from_vec(std::mem::take(&mut line)));
        if !path.is_absolute()
            || path.to_str().is_none()
            || path
                .components()
                .any(|part| part.as_os_str().to_string_lossy().starts_with('.'))
        {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if hits.len() == RESULT_LIMIT {
            return Ok((hits, false));
        }
        let path = NativePath::new(path);
        hits.push(SearchHit {
            name: path.name(),
            path,
            is_dir: metadata.is_dir(),
            volume_guid: None,
            volume_mount: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predicates_escape_quoted_terms_and_support_extensions() {
        assert_eq!(match_expression("ext:pdf"), "kMDItemFSName == \"*.pdf\"cd");
        assert_eq!(
            match_expression("월간 보고"),
            "kMDItemFSName == \"*월간 보고*\"cd"
        );
        assert_eq!(
            match_expression("a\"b\\c"),
            "kMDItemFSName == \"*a\\\"b\\\\c*\"cd"
        );
    }
}
