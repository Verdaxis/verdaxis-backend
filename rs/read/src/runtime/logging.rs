pub fn request_id(supplied: Option<&str>) -> String {
    match supplied {
        Some(value)
            if (1..=36).contains(&value.len())
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte)) =>
        {
            value.to_owned()
        }
        _ => uuid::Uuid::new_v4().to_string(),
    }
}
