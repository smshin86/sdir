use crate::model::NativePath;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Preview {
    Text { text: String, truncated: bool },
    Image { url: String },
}

#[cfg(windows)]
pub(crate) fn read(path: &NativePath) -> Result<Preview, String> {
    read_local(path, false)
}

#[cfg(windows)]
pub(crate) fn thumbnail(path: &NativePath) -> Option<String> {
    let Preview::Image { url } = read_local(path, true).ok()? else {
        return None;
    };
    Some(url)
}

#[cfg(not(windows))]
pub(crate) fn thumbnail(_: &NativePath) -> Option<String> {
    None
}

#[cfg(windows)]
fn read_local(path: &NativePath, thumbnail: bool) -> Result<Preview, String> {
    use crate::model::CloudState;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use std::{
        fs::{self, OpenOptions},
        io::Read,
        os::windows::fs::{MetadataExt, OpenOptionsExt},
        path::{Component, Path, Prefix},
    };
    use windows::{
        Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, GetDriveTypeW},
        core::PCWSTR,
    };

    fn fixed_disk(path: &Path) -> bool {
        let root = match path.components().next() {
            Some(Component::Prefix(p)) => match p.kind() {
                Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                    [u16::from(letter), 58, 92, 0]
                }
                _ => return false,
            },
            _ => return false,
        };
        path.is_absolute() && unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } == 3
    }

    fn allowed(meta: &fs::Metadata) -> bool {
        meta.is_file()
            && meta.file_attributes() & 0x400 == 0
            && CloudState::from_attributes(meta.file_attributes()) != CloudState::Remote
    }

    let path = path.to_path();
    let extension = path
        .extension()
        .and_then(|suffix| suffix.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let (limit, mime) = match extension.as_str() {
        "txt" | "md" | "log" | "csv" | "tsv" | "json" | "xml" | "ini" | "yaml" | "yml" => {
            (65536, None)
        }
        "png" => (524288, Some("image/png")),
        "jpg" | "jpeg" => (524288, Some("image/jpeg")),
        "webp" => (524288, Some("image/webp")),
        "mp4" | "mkv" | "mov" | "avi" | "wmv" | "webm" if thumbnail => (0, Some("video")),
        _ => return Err("텍스트, PNG, JPEG, WebP 파일을 미리 볼 수 있습니다.".into()),
    };
    if !fixed_disk(&path) {
        return Err("로컬 드라이브의 파일만 미리 볼 수 있습니다.".into());
    }
    let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if !allowed(&metadata) {
        return Err("온라인 전용 파일이나 연결 항목은 미리 읽지 않습니다.".into());
    }
    let resolved = fs::canonicalize(&path).map_err(|error| error.to_string())?;
    if !fixed_disk(&resolved) {
        return Err("연결된 원격 파일은 미리 읽지 않습니다.".into());
    }
    if thumbnail {
        if let Some(url) = cached_thumbnail(&resolved) {
            return Ok(Preview::Image { url });
        }
        if limit == 0 {
            return Err("저장된 영상 썸네일이 없습니다.".into());
        }
    }
    // Validate the opened object before reading. Opening a reparse point does not hydrate it.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(&resolved)
        .map_err(|error| error.to_string())?;
    if !allowed(&file.metadata().map_err(|error| error.to_string())?) {
        return Err("파일이 변경되었거나 미리 볼 수 없는 항목입니다.".into());
    }
    let mut data = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut data)
        .map_err(|error| error.to_string())?;
    if let Some(mime) = mime {
        if data.len() as u64 > limit {
            return Err("이미지 미리보기 한도는 512 KiB입니다.".into());
        }
        let reader = || image::ImageReader::new(std::io::Cursor::new(&data)).with_guessed_format();
        let (width, height) = reader()
            .map_err(|error| error.to_string())?
            .into_dimensions()
            .map_err(|error| error.to_string())?;
        if width > 8192 || height > 8192 || u64::from(width) * u64::from(height) > 12_000_000 {
            return Err("이미지 미리보기 해상도 한도를 넘었습니다.".into());
        }
        if thumbnail {
            let image = reader()
                .map_err(|error| error.to_string())?
                .decode()
                .map_err(|error| error.to_string())?;
            let thumb = image.thumbnail(96, 96);
            let mut encoded = std::io::Cursor::new(Vec::new());
            thumb
                .write_to(&mut encoded, image::ImageFormat::Png)
                .map_err(|error| error.to_string())?;
            return Ok(Preview::Image {
                url: format!(
                    "data:image/png;base64,{}",
                    STANDARD.encode(encoded.into_inner())
                ),
            });
        }
        return Ok(Preview::Image {
            url: format!("data:{mime};base64,{}", STANDARD.encode(data)),
        });
    }
    let truncated = data.len() as u64 > limit;
    data.truncate(limit as usize);
    let data = data.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&data);
    if data.contains(&0) {
        return Err("UTF-8 텍스트 파일만 내용을 미리 볼 수 있습니다.".into());
    }
    let text = match std::str::from_utf8(data) {
        Ok(text) => text,
        Err(error) if truncated && error.error_len().is_none() => {
            std::str::from_utf8(&data[..error.valid_up_to()]).map_err(|error| error.to_string())?
        }
        Err(_) => return Err("UTF-8 텍스트 파일만 내용을 미리 볼 수 있습니다.".into()),
    };
    Ok(Preview::Text {
        text: text.to_owned(),
        truncated,
    })
}

#[cfg(not(windows))]
pub(crate) fn read(_: &NativePath) -> Result<Preview, String> {
    Err("Windows 전용 기능입니다.".into())
}

#[cfg(windows)]
fn cached_thumbnail(path: &std::path::Path) -> Option<String> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use std::{mem::size_of, os::windows::ffi::OsStrExt};
    use windows::{
        Win32::{
            Foundation::SIZE,
            Graphics::Gdi::{
                BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC,
                GetDIBits, GetObjectW, HBITMAP, HDC, ReleaseDC,
            },
            System::Ole::{OleInitialize, OleUninitialize},
            UI::Shell::{
                IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_INCACHEONLY,
                SIIGBF_THUMBNAILONLY,
            },
        },
        core::PCWSTR,
    };
    struct Ole;
    impl Drop for Ole {
        fn drop(&mut self) {
            unsafe { OleUninitialize() };
        }
    }
    struct Bitmap(HBITMAP);
    impl Drop for Bitmap {
        fn drop(&mut self) {
            let _ = unsafe { DeleteObject(self.0.into()) };
        }
    }
    struct DeviceContext(HDC);
    impl Drop for DeviceContext {
        fn drop(&mut self) {
            let _ = unsafe { ReleaseDC(None, self.0) };
        }
    }
    let wide: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe { OleInitialize(None) }.ok()?;
    let _ole = Ole;
    let shell: IShellItemImageFactory =
        unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None) }.ok()?;
    let bitmap = Bitmap(
        unsafe {
            shell.GetImage(
                SIZE { cx: 96, cy: 96 },
                SIIGBF_INCACHEONLY | SIIGBF_THUMBNAILONLY,
            )
        }
        .ok()?,
    );
    let mut info = BITMAP::default();
    if unsafe {
        GetObjectW(
            bitmap.0.into(),
            size_of::<BITMAP>() as i32,
            Some((&mut info as *mut BITMAP).cast()),
        )
    } == 0
        || !(1..=256).contains(&info.bmWidth)
        || !(1..=256).contains(&info.bmHeight)
    {
        return None;
    }
    let dc = DeviceContext(unsafe { GetDC(None) });
    if dc.0.0.is_null() {
        return None;
    }
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: info.bmWidth,
            biHeight: -info.bmHeight,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut rgba = vec![0u8; (info.bmWidth * info.bmHeight * 4) as usize];
    if unsafe {
        GetDIBits(
            dc.0,
            bitmap.0,
            0,
            info.bmHeight as u32,
            Some(rgba.as_mut_ptr().cast()),
            &mut bmi,
            DIB_RGB_COLORS,
        )
    } != info.bmHeight
    {
        return None;
    }
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        pixel[3] = 255;
    }
    let image = image::RgbaImage::from_raw(info.bmWidth as u32, info.bmHeight as u32, rgba)?;
    let mut encoded = std::io::Cursor::new(Vec::new());
    image.write_to(&mut encoded, image::ImageFormat::Png).ok()?;
    Some(format!(
        "data:image/png;base64,{}",
        STANDARD.encode(encoded.into_inner())
    ))
}
