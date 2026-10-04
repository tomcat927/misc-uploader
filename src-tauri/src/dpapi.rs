// dpapi.rs — 本机配置密码字段的 Windows DPAPI 加密(零第三方依赖,直接 FFI crypt32)。
//
// 存储格式:`dpapi:v1:` + hex(CryptProtectData 输出)。blob 绑定「当前 Windows 用户档案 +
// 应用熵(ENTROPY)」,同机同用户可解,跨机器/换档案不可解(密码需重新输入,这是 DPAPI 的
// 语义而非故障)。读回兼容三个时代的值:旧明文(无前缀)原样使用;合法 blob 解密;
// 恰好带前缀但不是合法 hex 的明文密码 → 原样兜底。非 Windows 编译目标回退为不加密。

pub const PREFIX: &str = "dpapi:v1:";

// 应用熵:把 blob 圈定在本应用,其他进程的 DPAPI 数据与本字段互不可解
const ENTROPY: &str = "misc-uploader/config-password/v1";

pub fn encrypt_or_plain(plain: &str) -> String {
    encrypt(plain).unwrap_or_else(|| plain.to_string())
}

/// 读回:加密 blob → 解密;旧明文/无法识别 → 原样。解密失败(跨机器等)返回空串,由调用方提示重输。
pub fn decrypt_or_passthrough(stored: &str) -> String {
    if stored.starts_with(PREFIX) && stored[PREFIX.len()..].bytes().all(|b| b.is_ascii_hexdigit()) {
        decrypt_stored(stored).unwrap_or_default()
    } else {
        stored.to_string()
    }
}

fn to_hex(bytes: &[u8]) -> String {
    const T: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(T[(b >> 4) as usize] as char);
        s.push(T[(b & 15) as usize] as char);
    }
    s
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let byte = |pair: &[u8]| -> Option<u8> {
        let hi = (pair[0] as char).to_digit(16)? as u8;
        let lo = (pair[1] as char).to_digit(16)? as u8;
        Some(hi << 4 | lo)
    };
    s.as_bytes().chunks(2).map(byte).collect()
}

#[cfg(windows)]
mod ffi {
    use std::os::windows::ffi::OsStrExt;

    pub const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;

    #[repr(C)]
    pub struct CryptBlob {
        pub cb_data: u32,
        pub pb_data: *mut u8,
    }

    #[link(name = "crypt32")]
    unsafe extern "system" {
        pub fn CryptProtectData(
            data_in: *const CryptBlob,
            data_descr: *const u16,
            optional_entropy: *const CryptBlob,
            reserved: *mut core::ffi::c_void,
            prompt: *mut core::ffi::c_void,
            flags: u32,
            data_out: *mut CryptBlob,
        ) -> i32;
        pub fn CryptUnprotectData(
            data_in: *const CryptBlob,
            data_descr: *mut *mut u16,
            optional_entropy: *const CryptBlob,
            reserved: *mut core::ffi::c_void,
            prompt: *mut core::ffi::c_void,
            flags: u32,
            data_out: *mut CryptBlob,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn LocalFree(hmem: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
    }

    pub fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }
}

#[cfg(windows)]
fn protect(plain: &[u8]) -> Option<Vec<u8>> {
    use ffi::*;
    let entropy = wide(ENTROPY);
    let data_in = CryptBlob { cb_data: plain.len() as u32, pb_data: plain.as_ptr() as *mut u8 };
    let eb = CryptBlob { cb_data: (entropy.len() * 2) as u32, pb_data: entropy.as_ptr() as *mut u8 };
    let mut out = CryptBlob { cb_data: 0, pb_data: std::ptr::null_mut() };
    let ok = unsafe {
        CryptProtectData(
            &data_in,
            std::ptr::null(),
            &eb,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    };
    if ok == 0 || out.pb_data.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(out.pb_data, out.cb_data as usize) }.to_vec();
    unsafe { LocalFree(out.pb_data.cast()) };
    Some(bytes)
}

#[cfg(windows)]
fn unprotect(blob: &[u8]) -> Option<Vec<u8>> {
    use ffi::*;
    let entropy = wide(ENTROPY);
    let data_in = CryptBlob { cb_data: blob.len() as u32, pb_data: blob.as_ptr() as *mut u8 };
    let eb = CryptBlob { cb_data: (entropy.len() * 2) as u32, pb_data: entropy.as_ptr() as *mut u8 };
    let mut out = CryptBlob { cb_data: 0, pb_data: std::ptr::null_mut() };
    let ok = unsafe {
        CryptUnprotectData(
            &data_in,
            std::ptr::null_mut(),
            &eb,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    };
    if ok == 0 || out.pb_data.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(out.pb_data, out.cb_data as usize) }.to_vec();
    unsafe { LocalFree(out.pb_data.cast()) };
    Some(bytes)
}

#[cfg(windows)]
fn encrypt(plain: &str) -> Option<String> {
    protect(plain.as_bytes()).map(|b| format!("{PREFIX}{}", to_hex(&b)))
}

#[cfg(windows)]
fn decrypt_stored(stored: &str) -> Option<String> {
    let hexpart = stored.strip_prefix(PREFIX)?;
    let blob = from_hex(hexpart)?;
    let plain = unprotect(&blob)?;
    String::from_utf8(plain).ok()
}

#[cfg(not(windows))]
fn encrypt(_plain: &str) -> Option<String> {
    None // 非 Windows:不加密,明文落盘(与旧行为一致);本应用仅发布 Windows 包
}

#[cfg(not(windows))]
fn decrypt_stored(_stored: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let s = to_hex(&[0x00, 0xde, 0xad, 0xbe, 0xef, 0xff]);
        assert_eq!(s, "00deadbeefff");
        assert_eq!(from_hex(&s).unwrap(), vec![0x00, 0xde, 0xad, 0xbe, 0xef, 0xff]);
        assert!(from_hex("abc").is_none());
        assert!(from_hex("zz").is_none());
    }

    #[test]
    fn passthrough_rules() {
        assert_eq!(decrypt_or_passthrough("旧明文密码"), "旧明文密码");
        assert_eq!(decrypt_or_passthrough(""), "");
        // 带前缀但非合法 hex → 按字面明文兜底
        assert_eq!(decrypt_or_passthrough("dpapi:v1:not-hex!"), "dpapi:v1:not-hex!");
        // 合法 hex 但解不开(测试环境无真实 blob)→ 空
        assert_eq!(decrypt_or_passthrough("dpapi:v1:00ff"), "");
    }
}
