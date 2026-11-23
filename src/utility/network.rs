use windows::{
    Win32::NetworkManagement::WNet::{
        CONNECT_UPDATE_PROFILE, NET_CONNECT_FLAGS, NETRESOURCEW, RESOURCETYPE_DISK,
        WNetAddConnection2W, WNetCancelConnection2W, WNetGetConnectionW,
    },
    core::{PCWSTR, PWSTR},
};

pub(super) fn change(drive: &str, remote: Option<&str>, remember: bool) -> Result<(), String> {
    super::validate_network_drive(drive, remote)?;
    let mut local: Vec<_> = drive.encode_utf16().chain(Some(0)).collect();
    let flags = if remember {
        CONNECT_UPDATE_PROFILE
    } else {
        NET_CONNECT_FLAGS(0)
    };
    let result = if let Some(remote) = remote {
        let mut address: Vec<_> = remote.encode_utf16().chain(Some(0)).collect();
        let resource = NETRESOURCEW {
            dwType: RESOURCETYPE_DISK,
            lpLocalName: PWSTR(local.as_mut_ptr()),
            lpRemoteName: PWSTR(address.as_mut_ptr()),
            ..Default::default()
        };
        unsafe { WNetAddConnection2W(&resource, PCWSTR::null(), PCWSTR::null(), flags) }
    } else {
        let drive = PCWSTR(local.as_ptr());
        let mut remote = vec![0u16; 4096];
        let mut capacity = remote.len() as u32;
        let connection =
            unsafe { WNetGetConnectionW(drive, Some(PWSTR(remote.as_mut_ptr())), &mut capacity) };
        use windows::Win32::Foundation::{ERROR_CONNECTION_UNAVAIL, NO_ERROR};
        if connection != NO_ERROR && connection != ERROR_CONNECTION_UNAVAIL {
            return Err("연결되거나 저장된 네트워크 드라이브 문자를 입력하세요.".into());
        }
        unsafe { WNetCancelConnection2W(drive, CONNECT_UPDATE_PROFILE, false) }
    };
    if result.0 == 0 {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(result.0 as i32).to_string())
    }
}
