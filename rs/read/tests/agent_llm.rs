//! Public transport admission needs neither a database nor provider access.
use verdaxis_read::agent::llm::{ErrorCode, models};

#[test]
fn mistral_model_allow_list_is_closed() {
    assert_eq!(models::MODELS.len(), 1);
    assert!(models::settings("deepseek-v4-1-flash-1017").is_ok());
    assert!(matches!(
        models::settings(concat!("deepseek-v4", ".1-flash")),
        Err(ErrorCode::ModelNotAllowed)
    ));
}
