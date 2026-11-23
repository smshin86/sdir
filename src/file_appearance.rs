use crate::model::NativePath;
use serde::{Deserialize, Serialize};

pub const BATCH_LIMIT: usize = 32;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct FileAppearanceKey {
    pub extension: String,
    pub embedded: Option<EmbeddedIcon>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct EmbeddedIcon {
    pub path: NativePath,
    pub bytes: Option<u64>,
    pub modified: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FileAppearance {
    pub type_name: String,
    pub small: Option<String>,
    pub large: Option<String>,
}

#[cfg(windows)]
pub(crate) fn load(keys: &[FileAppearanceKey]) -> Vec<Option<FileAppearance>> {
    use windows::Win32::System::Ole::{OleInitialize, OleUninitialize};
    if unsafe { OleInitialize(None) }.is_err() {
        return Vec::new();
    }
    let result = keys.iter().take(BATCH_LIMIT).map(native::load).collect();
    unsafe { OleUninitialize() };
    result
}

#[cfg(target_os = "macos")]
pub(crate) fn load(keys: &[FileAppearanceKey]) -> Vec<Option<FileAppearance>> {
    keys.iter()
        .take(BATCH_LIMIT)
        .map(|key| Some(crate::macos::appearance(key)))
        .collect()
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) fn load(_: &[FileAppearanceKey]) -> Vec<Option<FileAppearance>> {
    Vec::new()
}

#[cfg(windows)]
mod native {
    use super::{FileAppearance, FileAppearanceKey};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use std::{
        ffi::c_void,
        mem::size_of,
        os::windows::fs::MetadataExt,
        path::{Component, Path, Prefix},
    };
    use windows::{
        Win32::{
            Graphics::Gdi::{
                BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection,
                DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, SelectObject,
            },
            Storage::FileSystem::{
                FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, GetDriveTypeW,
            },
            UI::{
                Shell::{
                    ExtractIconExW, SHFILEINFOW, SHGFI_FLAGS, SHGFI_ICON, SHGFI_LARGEICON,
                    SHGFI_SMALLICON, SHGFI_TYPENAME, SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW,
                },
                WindowsAndMessaging::{DI_NORMAL, DestroyIcon, DrawIconEx, HICON},
            },
        },
        core::PCWSTR,
    };

    struct Icon(HICON);
    impl Drop for Icon {
        fn drop(&mut self) {
            if !self.0.0.is_null() {
                let _ = unsafe { DestroyIcon(self.0) };
            }
        }
    }

    fn shell_icon(filename: &[u16], size: SHGFI_FLAGS) -> (String, Icon) {
        let mut info = SHFILEINFOW::default();
        let result = unsafe {
            SHGetFileInfoW(
                PCWSTR(filename.as_ptr()),
                FILE_ATTRIBUTE_NORMAL,
                Some(&mut info),
                size_of::<SHFILEINFOW>() as u32,
                size | SHGFI_USEFILEATTRIBUTES | SHGFI_ICON | SHGFI_TYPENAME,
            )
        };
        let icon = Icon(info.hIcon);
        if result == 0 {
            return (String::new(), Icon(HICON::default()));
        }
        let len = info
            .szTypeName
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(info.szTypeName.len());
        (String::from_utf16_lossy(&info.szTypeName[..len]), icon)
    }

    fn local_regular(path: &Path) -> bool {
        let Some(Component::Prefix(prefix)) = path.components().next() else {
            return false;
        };
        let (Prefix::Disk(letter) | Prefix::VerbatimDisk(letter)) = prefix.kind() else {
            return false;
        };
        let root = [u16::from(letter), b':' as u16, b'\\' as u16, 0];
        path.is_absolute()
            && unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) == 3 }
            && std::fs::symlink_metadata(path).is_ok_and(|meta| {
                let attributes = meta.file_attributes();
                meta.is_file()
                    && attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0
                    && crate::model::CloudState::from_attributes(attributes)
                        == crate::model::CloudState::None
            })
    }

    pub(super) fn load(key: &FileAppearanceKey) -> Option<FileAppearance> {
        let extension = &key.extension;
        if extension.len() > 1020 || extension.contains(['\0', '.', ':', '/', '\\']) {
            return None;
        }
        let mut filename: Vec<u16> = "sdir-file".encode_utf16().collect();
        if !extension.is_empty() {
            filename.push(b'.' as u16);
            filename.extend(extension.encode_utf16());
        }
        filename.push(0);
        let (type_name, small) = shell_icon(&filename, SHGFI_SMALLICON);
        let (_, large) = shell_icon(&filename, SHGFI_LARGEICON);
        let mut custom_small = Icon(HICON::default());
        let mut custom_large = Icon(HICON::default());
        if let Some(source) = key.embedded.as_ref().filter(|source| {
            matches!(extension.as_str(), "exe" | "ico")
                && local_regular(&source.path.to_path())
                && source
                    .path
                    .to_path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
        }) {
            let path: Vec<u16> = source.path.0.iter().copied().chain([0]).collect();
            unsafe {
                ExtractIconExW(
                    PCWSTR(path.as_ptr()),
                    0,
                    Some(&mut custom_large.0),
                    Some(&mut custom_small.0),
                    1,
                )
            };
        }
        Some(FileAppearance {
            type_name,
            small: to_png(&custom_small, 16).or_else(|| to_png(&small, 16)),
            large: to_png(&custom_large, 32).or_else(|| to_png(&large, 32)),
        })
    }

    fn to_png(icon: &Icon, size: i32) -> Option<String> {
        if icon.0.0.is_null() {
            return None;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let dc = unsafe { CreateCompatibleDC(None) };
        if dc.0.is_null() {
            return None;
        }
        let mut bits: *mut c_void = std::ptr::null_mut();
        let result = (|| {
            let bitmap =
                unsafe { CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) }
                    .ok()?;
            let previous = unsafe { SelectObject(dc, bitmap.into()) };
            let rendered = if bits.is_null() || previous.0.is_null() || previous.0 as isize == -1 {
                None
            } else {
                let pixels = unsafe {
                    std::slice::from_raw_parts_mut(bits.cast::<u8>(), (size * size * 4) as usize)
                };
                let mut render = |background| {
                    pixels.fill(background);
                    unsafe { DrawIconEx(dc, 0, 0, icon.0, size, size, 0, None, DI_NORMAL) }.ok()?;
                    let _ = unsafe { GdiFlush() };
                    Some(pixels.to_vec())
                };
                render(0).zip(render(255))
            };
            if !previous.0.is_null() && previous.0 as isize != -1 {
                unsafe { SelectObject(dc, previous) };
            }
            let _ = unsafe { DeleteObject(bitmap.into()) };
            rendered
        })();
        let _ = unsafe { DeleteDC(dc) };
        let (mut black, white) = result?;
        // Compare opaque backgrounds to preserve both antialiased alpha and legacy AND masks.
        for (dark, light) in black.chunks_exact_mut(4).zip(white.chunks_exact(4)) {
            let alpha = 255 - (0..3).map(|i| light[i].saturating_sub(dark[i])).min()?;
            if alpha == 0 {
                dark.fill(0);
                continue;
            }
            for channel in &mut dark[..3] {
                *channel = ((u16::from(*channel) * 255 + u16::from(alpha) / 2) / u16::from(alpha))
                    .min(255) as u8;
            }
            dark.swap(0, 2);
            dark[3] = alpha;
        }
        let img = image::RgbaImage::from_raw(size as u32, size as u32, black)?;
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).ok()?;
        Some(format!(
            "data:image/png;base64,{}",
            STANDARD.encode(png.into_inner())
        ))
    }
}
