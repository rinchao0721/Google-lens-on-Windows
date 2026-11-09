//! UTF-16 字符串辅助。

/// 转为以 NUL 结尾的 UTF-16。
pub(crate) fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 复制到定长缓冲区，超长截断，并保证以 NUL 结尾。
pub(crate) fn copy_wide<const N: usize>(dst: &mut [u16; N], text: &str) {
    dst.fill(0);
    let limit = N.saturating_sub(1);
    for (slot, unit) in dst.iter_mut().zip(text.encode_utf16().take(limit)) {
        *slot = unit;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_wide_appends_nul() {
        assert_eq!(to_wide("ab"), vec![97, 98, 0]);
        assert_eq!(to_wide(""), vec![0]);
    }

    #[test]
    fn copy_wide_truncates_and_terminates() {
        let mut buf = [9u16; 4];
        copy_wide(&mut buf, "abcdef");
        assert_eq!(buf, [97, 98, 99, 0]);
        copy_wide(&mut buf, "a");
        assert_eq!(buf, [97, 0, 0, 0]);
    }

    #[test]
    fn copy_wide_handles_chinese() {
        let mut buf = [0u16; 8];
        copy_wide(&mut buf, "已在运行");
        assert_eq!(buf.iter().take_while(|u| **u != 0).count(), 4);
    }
}
