use crate::model::NativePath;
use std::{fs, io::Write, os::windows::ffi::OsStrExt};
use windows::{
    Win32::{
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistStream, STATFLAG_NONAME, STATSTG,
                STREAM_SEEK_SET,
            },
            Ole::{OleInitialize, OleUninitialize},
        },
        UI::Shell::{IShellLinkW, SHCreateMemStream, ShellLink},
    },
    core::{Interface, PCWSTR},
};

pub(super) fn create(folder: &NativePath, paths: &[NativePath]) -> Result<(), String> {
    if !(1..=500).contains(&paths.len()) || !folder.to_path().is_dir() {
        return Err("바로 가기를 만들 폴더와 항목 1~500개를 선택하세요.".into());
    }
    unsafe { OleInitialize(None) }.map_err(|error| error.to_string())?;
    struct Ole;
    impl Drop for Ole {
        fn drop(&mut self) {
            unsafe { OleUninitialize() };
        }
    }
    let _ole = Ole;
    let mut created = 0;
    for path in paths {
        if let Err(error) = create_one(folder, path) {
            return Err(format!(
                "바로 가기 {created}개 생성 · 다음 항목을 만들지 못했습니다: {error}"
            ));
        }
        created += 1;
    }
    Ok(())
}

fn create_one(folder: &NativePath, source: &NativePath) -> Result<(), String> {
    let source = source.to_path();
    let name = source
        .file_name()
        .ok_or("루트의 바로 가기는 여기서 만들 수 없습니다.")?;
    if !source.exists() {
        return Err("원본 항목을 찾을 수 없습니다.".into());
    }
    let wide: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let bytes = unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| error.to_string())?;
        link.SetPath(PCWSTR(wide.as_ptr()))
            .map_err(|error| error.to_string())?;
        let persistence: IPersistStream = link.cast().map_err(|error| error.to_string())?;
        let stream = SHCreateMemStream(None).ok_or("바로 가기 데이터를 준비하지 못했습니다.")?;
        persistence
            .Save(&stream, true)
            .map_err(|error| error.to_string())?;
        let mut stat = STATSTG::default();
        stream
            .Stat(&mut stat, STATFLAG_NONAME)
            .map_err(|error| error.to_string())?;
        if stat.cbSize == 0 || stat.cbSize > 1024 * 1024 {
            return Err("바로 가기 데이터 크기가 유효하지 않습니다.".into());
        }
        stream
            .Seek(0, STREAM_SEEK_SET, None)
            .map_err(|error| error.to_string())?;
        let mut bytes = vec![0; stat.cbSize as usize];
        let mut read = 0;
        stream
            .Read(
                bytes.as_mut_ptr().cast(),
                bytes.len() as u32,
                Some(&mut read),
            )
            .ok()
            .map_err(|error| error.to_string())?;
        if read as usize != bytes.len() {
            return Err("바로 가기 데이터를 끝까지 읽지 못했습니다.".into());
        }
        bytes
    };
    let directory = folder.to_path();
    for number in 1..=200 {
        let mut link_name = name.to_os_string();
        link_name.push(" - 바로 가기");
        if number > 1 {
            link_name.push(format!(" ({number})"));
        }
        link_name.push(".lnk");
        let path = directory.join(link_name);
        let mut file = match fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        };
        if let Err(error) = file.write_all(&bytes) {
            drop(file);
            let _ = fs::remove_file(path);
            return Err(error.to_string());
        }
        return Ok(());
    }
    Err("같은 이름의 바로 가기가 이미 많이 있습니다.".into())
}
