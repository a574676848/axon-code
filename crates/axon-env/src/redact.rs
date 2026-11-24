//! 脱敏：email 只保留首字母与域名。

/// 把 email 脱敏为 `首字母***@域名`；无法识别时返回 `***`。
///
/// 例：`alice@example.com` → `a***@example.com`。
#[must_use]
pub fn redact_email(email: &str) -> String {
    let email = email.trim();
    let Some((local, domain)) = email.split_once('@') else {
        return "***".to_owned();
    };
    let domain = domain.trim();
    if domain.is_empty() {
        return "***".to_owned();
    }
    match local.trim().chars().next() {
        Some(first) => format!("{first}***@{domain}"),
        None => format!("***@{domain}"),
    }
}

#[cfg(test)]
mod tests {
    use super::redact_email;

    #[test]
    fn keeps_first_char_and_domain() {
        assert_eq!(redact_email("alice@example.com"), "a***@example.com");
        assert_eq!(redact_email("  bob@corp.cn  "), "b***@corp.cn");
    }

    #[test]
    fn chinese_local_part_keeps_first_char() {
        assert_eq!(redact_email("张三@example.com"), "张***@example.com");
    }

    #[test]
    fn malformed_email_becomes_placeholder() {
        assert_eq!(redact_email("not-an-email"), "***");
        assert_eq!(redact_email("alice@"), "***");
        assert_eq!(redact_email(""), "***");
    }
}
