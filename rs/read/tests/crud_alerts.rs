use serde_json::{Value, json};
use sqlx::Connection;
use uuid::Uuid;
use verdaxis_read::crud::alerts::{AlertCreate, authorize_delete, organization_id};
use verdaxis_read::runtime::artifact::ALLOWED_REVISIONS;

#[test]
fn foreign_alert_delete_is_403() {
    let own = Uuid::from_u128(1);
    let foreign = Uuid::from_u128(2);
    let error = authorize_delete(own, Some(foreign)).unwrap_err();
    assert_eq!(error.status, 403);
    assert_eq!(error.detail, "You can only delete your own alerts");
    assert_eq!(authorize_delete(own, None).unwrap_err().status, 404);
    assert!(authorize_delete(own, Some(own)).is_ok());
}

#[test]
fn missing_organization_is_400() {
    let error = organization_id(None).unwrap_err();
    assert_eq!(error.status, 400);
    assert_eq!(error.detail, "User must belong to an organization");
}

#[test]
fn request_validation_matches_python_http_vectors() {
    let fixture: Value = serde_json::from_str(r####"{"version":1,"fixture_only":true,"fastapi_version":"0.135.3","pydantic_version":"2.13.4","settings":{"environment":"test","jwt_secret_env":"JWT_SECRET","jwt_issuer_env":"JWT_ISSUER","jwt_audience_env":"JWT_AUDIENCE","free_tier_alert_limit":5},"sources":{"app/routers/alerts.py":"898db5e873ecf55047e8bff89a01ab6b5aae7511593011314301d0f6fed84129","app/schemas/alerts.py":"79f4afd7d2be38ace0e03c3fbce76d0721e22155a494282dcdb1e059f1ac9296","app/models/alerts.py":"a7b2230941ee3554df0859e5f39e1cc14dc5d5c96a0bc797a55be2eeb8ec43c1"},"python_behaviour":{"A01":{"success_status":201,"no_organization":{"status":400,"detail":"User must belong to an organization"},"free_tier_reached":{"status":403,"detail":"Free tier limit of 5 active alerts reached. Delete an existing alert to create a new one."},"database_failure":{"status":500,"detail":"Database operation failed."}},"A02":{"success_status":200,"no_organization":{"status":400,"detail":"User must belong to an organization"},"order":"created_at DESC, no secondary key, inactive rows included"},"A03":{"success_status":204,"no_organization":{"status":400,"detail":"User must belong to an organization"},"not_found":{"status":404,"detail":"Alert not found"},"foreign_organization":{"status":403,"detail":"You can only delete your own alerts"}}},"admission_order":["json_syntax_422","authentication_401","typed_validation_422","handler_400_403_404"],"response_examples":{"single":"{\"id\":\"50000000-0000-4000-8000-000000000001\",\"org_id\":\"10000000-0000-4000-8000-000000000001\",\"product_id\":\"30000000-0000-4000-8000-000000000001\",\"product_name\":\"VLSFO 0.5% Singapore\",\"market_product\":\"VLSFO_SINGAPORE\",\"delivery_point_id\":\"40000000-0000-4000-8000-000000000001\",\"direction\":\"above\",\"threshold_usd\":\"12.34\",\"is_active\":true,\"triggered_at\":\"2026-10-18T12:30:00Z\",\"created_at\":\"2026-10-18T12:00:00Z\"}","list":"[{\"id\":\"50000000-0000-4000-8000-000000000001\",\"org_id\":\"10000000-0000-4000-8000-000000000001\",\"product_id\":\"30000000-0000-4000-8000-000000000001\",\"product_name\":\"VLSFO 0.5% Singapore\",\"market_product\":\"VLSFO_SINGAPORE\",\"delivery_point_id\":\"40000000-0000-4000-8000-000000000001\",\"direction\":\"above\",\"threshold_usd\":\"12.34\",\"is_active\":true,\"triggered_at\":\"2026-10-18T12:30:00Z\",\"created_at\":\"2026-10-18T12:00:00Z\"},{\"id\":\"50000000-0000-4000-8000-000000000002\",\"org_id\":\"10000000-0000-4000-8000-000000000001\",\"product_id\":\"30000000-0000-4000-8000-000000000001\",\"product_name\":null,\"market_product\":null,\"delivery_point_id\":null,\"direction\":\"below\",\"threshold_usd\":\"0.01\",\"is_active\":false,\"triggered_at\":null,\"created_at\":\"2026-10-18T12:00:00Z\"}]"},"vectors":[{"id":"valid-full","field":"request","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"},"python":{"kind":"accepted","values":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"}}},{"id":"valid-null-point","field":"request","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":null,"direction":"above","threshold_usd":"12.34"},"python":{"kind":"accepted","values":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":null,"direction":"above","threshold_usd":"12.34"}}},{"id":"valid-point-absent","field":"request","body":{"product_id":"30000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"},"python":{"kind":"accepted","values":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":null,"direction":"above","threshold_usd":"12.34"}}},{"id":"valid-below","field":"request","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"below","threshold_usd":"0.01"},"python":{"kind":"accepted","values":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"below","threshold_usd":"0.01"}}},{"id":"valid-integer-threshold","field":"request","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":1},"python":{"kind":"accepted","values":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"1"}}},{"id":"valid-integer-string-threshold","field":"request","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"1"},"python":{"kind":"accepted","values":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"1"}}},{"id":"valid-float-threshold","field":"request","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":1.5},"python":{"kind":"accepted","values":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"1.5"}}},{"id":"valid-exponent-threshold","field":"request","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"1e2"},"python":{"kind":"accepted","values":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"100"}}},{"id":"valid-tiny-threshold","field":"request","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"0.0001"},"python":{"kind":"accepted","values":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"0.0001"}}},{"id":"valid-extra-field-ignored","field":"request","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34","is_active":false,"org_id":"10000000-0000-4000-8000-000000000001"},"python":{"kind":"accepted","values":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"}}},{"id":"missing-all-required","field":"missing","body":{},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"missing\",\"loc\":[\"body\",\"product_id\"],\"msg\":\"Field required\",\"input\":{}},{\"type\":\"missing\",\"loc\":[\"body\",\"direction\"],\"msg\":\"Field required\",\"input\":{}},{\"type\":\"missing\",\"loc\":[\"body\",\"threshold_usd\"],\"msg\":\"Field required\",\"input\":{}}]}"}},{"id":"missing-product-id","field":"product_id","body":{"delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"missing\",\"loc\":[\"body\",\"product_id\"],\"msg\":\"Field required\",\"input\":{\"delivery_point_id\":\"40000000-0000-4000-8000-000000000001\",\"direction\":\"above\",\"threshold_usd\":\"12.34\"}}]}"}},{"id":"missing-direction","field":"direction","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"missing\",\"loc\":[\"body\",\"direction\"],\"msg\":\"Field required\",\"input\":{\"product_id\":\"30000000-0000-4000-8000-000000000001\",\"delivery_point_id\":\"40000000-0000-4000-8000-000000000001\",\"threshold_usd\":\"12.34\"}}]}"}},{"id":"missing-threshold","field":"threshold_usd","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"missing\",\"loc\":[\"body\",\"threshold_usd\"],\"msg\":\"Field required\",\"input\":{\"product_id\":\"30000000-0000-4000-8000-000000000001\",\"delivery_point_id\":\"40000000-0000-4000-8000-000000000001\",\"direction\":\"above\"}}]}"}},{"id":"product-id-uuid-invalid","field":"product_id","body":{"product_id":"not-a-uuid","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"uuid_parsing\",\"loc\":[\"body\",\"product_id\"],\"msg\":\"Input should be a valid UUID, invalid character: found `n` at 1\",\"input\":\"not-a-uuid\",\"ctx\":{\"error\":\"invalid character: found `n` at 1\"}}]}"}},{"id":"delivery-point-id-uuid-invalid","field":"delivery_point_id","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"not-a-uuid","direction":"above","threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"uuid_parsing\",\"loc\":[\"body\",\"delivery_point_id\"],\"msg\":\"Input should be a valid UUID, invalid character: found `n` at 1\",\"input\":\"not-a-uuid\",\"ctx\":{\"error\":\"invalid character: found `n` at 1\"}}]}"}},{"id":"product-id-uuid-short","field":"product_id","body":{"product_id":"bad","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"uuid_parsing\",\"loc\":[\"body\",\"product_id\"],\"msg\":\"Input should be a valid UUID, invalid length: expected length 32 for simple format, found 3\",\"input\":\"bad\",\"ctx\":{\"error\":\"invalid length: expected length 32 for simple format, found 3\"}}]}"}},{"id":"delivery-point-id-uuid-short","field":"delivery_point_id","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"bad","direction":"above","threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"uuid_parsing\",\"loc\":[\"body\",\"delivery_point_id\"],\"msg\":\"Input should be a valid UUID, invalid length: expected length 32 for simple format, found 3\",\"input\":\"bad\",\"ctx\":{\"error\":\"invalid length: expected length 32 for simple format, found 3\"}}]}"}},{"id":"product-id-uuid-empty","field":"product_id","body":{"product_id":"","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"uuid_parsing\",\"loc\":[\"body\",\"product_id\"],\"msg\":\"Input should be a valid UUID, invalid length: expected length 32 for simple format, found 0\",\"input\":\"\",\"ctx\":{\"error\":\"invalid length: expected length 32 for simple format, found 0\"}}]}"}},{"id":"delivery-point-id-uuid-empty","field":"delivery_point_id","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"","direction":"above","threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"uuid_parsing\",\"loc\":[\"body\",\"delivery_point_id\"],\"msg\":\"Input should be a valid UUID, invalid length: expected length 32 for simple format, found 0\",\"input\":\"\",\"ctx\":{\"error\":\"invalid length: expected length 32 for simple format, found 0\"}}]}"}},{"id":"product-id-uuid-null","field":"product_id","body":{"product_id":null,"delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"uuid_type\",\"loc\":[\"body\",\"product_id\"],\"msg\":\"UUID input should be a string, bytes or UUID object\",\"input\":null}]}"}},{"id":"delivery-point-id-uuid-null","field":"delivery_point_id","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":null,"direction":"above","threshold_usd":"12.34"},"python":{"kind":"accepted","values":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":null,"direction":"above","threshold_usd":"12.34"}}},{"id":"product-id-uuid-number","field":"product_id","body":{"product_id":123,"delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"uuid_type\",\"loc\":[\"body\",\"product_id\"],\"msg\":\"UUID input should be a string, bytes or UUID object\",\"input\":123}]}"}},{"id":"delivery-point-id-uuid-number","field":"delivery_point_id","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":123,"direction":"above","threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"uuid_type\",\"loc\":[\"body\",\"delivery_point_id\"],\"msg\":\"UUID input should be a string, bytes or UUID object\",\"input\":123}]}"}},{"id":"product-id-uuid-boolean","field":"product_id","body":{"product_id":true,"delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"uuid_type\",\"loc\":[\"body\",\"product_id\"],\"msg\":\"UUID input should be a string, bytes or UUID object\",\"input\":true}]}"}},{"id":"delivery-point-id-uuid-boolean","field":"delivery_point_id","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":true,"direction":"above","threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"uuid_type\",\"loc\":[\"body\",\"delivery_point_id\"],\"msg\":\"UUID input should be a string, bytes or UUID object\",\"input\":true}]}"}},{"id":"product-id-uuid-array","field":"product_id","body":{"product_id":[],"delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"uuid_type\",\"loc\":[\"body\",\"product_id\"],\"msg\":\"UUID input should be a string, bytes or UUID object\",\"input\":[]}]}"}},{"id":"delivery-point-id-uuid-array","field":"delivery_point_id","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":[],"direction":"above","threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"uuid_type\",\"loc\":[\"body\",\"delivery_point_id\"],\"msg\":\"UUID input should be a string, bytes or UUID object\",\"input\":[]}]}"}},{"id":"product-id-uuid-object","field":"product_id","body":{"product_id":{},"delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"uuid_type\",\"loc\":[\"body\",\"product_id\"],\"msg\":\"UUID input should be a string, bytes or UUID object\",\"input\":{}}]}"}},{"id":"delivery-point-id-uuid-object","field":"delivery_point_id","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":{},"direction":"above","threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"uuid_type\",\"loc\":[\"body\",\"delivery_point_id\"],\"msg\":\"UUID input should be a string, bytes or UUID object\",\"input\":{}}]}"}},{"id":"product-id-uuid-valid-simple-format","field":"product_id","body":{"product_id":"30000000000040008000000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"},"python":{"kind":"accepted","values":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"}}},{"id":"delivery-point-id-uuid-valid-simple-format","field":"delivery_point_id","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"30000000000040008000000000000001","direction":"above","threshold_usd":"12.34"},"python":{"kind":"accepted","values":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"30000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"}}},{"id":"product-id-uuid-valid-uppercase","field":"product_id","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"},"python":{"kind":"accepted","values":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"}}},{"id":"delivery-point-id-uuid-valid-uppercase","field":"delivery_point_id","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"30000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"},"python":{"kind":"accepted","values":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"30000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"}}},{"id":"product-id-uuid-valid-braces","field":"product_id","body":{"product_id":"{30000000-0000-4000-8000-000000000001}","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"},"python":{"kind":"accepted","values":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"}}},{"id":"delivery-point-id-uuid-valid-braces","field":"delivery_point_id","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"{30000000-0000-4000-8000-000000000001}","direction":"above","threshold_usd":"12.34"},"python":{"kind":"accepted","values":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"30000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"12.34"}}},{"id":"direction-invalid-sideways","field":"direction","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"sideways","threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"value_error\",\"loc\":[\"body\",\"direction\"],\"msg\":\"Value error, direction must be 'above' or 'below'\",\"input\":\"sideways\",\"ctx\":{\"error\":{}}}]}"}},{"id":"direction-invalid-case","field":"direction","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"Above","threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"value_error\",\"loc\":[\"body\",\"direction\"],\"msg\":\"Value error, direction must be 'above' or 'below'\",\"input\":\"Above\",\"ctx\":{\"error\":{}}}]}"}},{"id":"direction-invalid-upper","field":"direction","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"ABOVE","threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"value_error\",\"loc\":[\"body\",\"direction\"],\"msg\":\"Value error, direction must be 'above' or 'below'\",\"input\":\"ABOVE\",\"ctx\":{\"error\":{}}}]}"}},{"id":"direction-invalid-empty","field":"direction","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"","threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"value_error\",\"loc\":[\"body\",\"direction\"],\"msg\":\"Value error, direction must be 'above' or 'below'\",\"input\":\"\",\"ctx\":{\"error\":{}}}]}"}},{"id":"direction-invalid-number","field":"direction","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":123,"threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"string_type\",\"loc\":[\"body\",\"direction\"],\"msg\":\"Input should be a valid string\",\"input\":123}]}"}},{"id":"direction-invalid-null","field":"direction","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":null,"threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"string_type\",\"loc\":[\"body\",\"direction\"],\"msg\":\"Input should be a valid string\",\"input\":null}]}"}},{"id":"direction-invalid-boolean","field":"direction","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":true,"threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"string_type\",\"loc\":[\"body\",\"direction\"],\"msg\":\"Input should be a valid string\",\"input\":true}]}"}},{"id":"direction-invalid-array","field":"direction","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":[],"threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"string_type\",\"loc\":[\"body\",\"direction\"],\"msg\":\"Input should be a valid string\",\"input\":[]}]}"}},{"id":"direction-invalid-object","field":"direction","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":{},"threshold_usd":"12.34"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"string_type\",\"loc\":[\"body\",\"direction\"],\"msg\":\"Input should be a valid string\",\"input\":{}}]}"}},{"id":"threshold-zero","field":"threshold_usd","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":0},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"value_error\",\"loc\":[\"body\",\"threshold_usd\"],\"msg\":\"Value error, threshold_usd must be positive\",\"input\":0,\"ctx\":{\"error\":{}}}]}"}},{"id":"threshold-zero-string","field":"threshold_usd","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"0"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"value_error\",\"loc\":[\"body\",\"threshold_usd\"],\"msg\":\"Value error, threshold_usd must be positive\",\"input\":\"0\",\"ctx\":{\"error\":{}}}]}"}},{"id":"threshold-zero-decimal-string","field":"threshold_usd","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"0.00"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"value_error\",\"loc\":[\"body\",\"threshold_usd\"],\"msg\":\"Value error, threshold_usd must be positive\",\"input\":\"0.00\",\"ctx\":{\"error\":{}}}]}"}},{"id":"threshold-negative-integer","field":"threshold_usd","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":-1},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"value_error\",\"loc\":[\"body\",\"threshold_usd\"],\"msg\":\"Value error, threshold_usd must be positive\",\"input\":-1,\"ctx\":{\"error\":{}}}]}"}},{"id":"threshold-negative-string","field":"threshold_usd","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"-0.01"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"value_error\",\"loc\":[\"body\",\"threshold_usd\"],\"msg\":\"Value error, threshold_usd must be positive\",\"input\":\"-0.01\",\"ctx\":{\"error\":{}}}]}"}},{"id":"threshold-parsing-text","field":"threshold_usd","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"fast"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"decimal_parsing\",\"loc\":[\"body\",\"threshold_usd\"],\"msg\":\"Input should be a valid decimal\",\"input\":\"fast\"}]}"}},{"id":"threshold-parsing-empty","field":"threshold_usd","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":""},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"decimal_parsing\",\"loc\":[\"body\",\"threshold_usd\"],\"msg\":\"Input should be a valid decimal\",\"input\":\"\"}]}"}},{"id":"threshold-parsing-null","field":"threshold_usd","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":null},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"decimal_type\",\"loc\":[\"body\",\"threshold_usd\"],\"msg\":\"Decimal input should be an integer, float, string or Decimal object\",\"input\":null}]}"}},{"id":"threshold-parsing-boolean","field":"threshold_usd","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":true},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"decimal_type\",\"loc\":[\"body\",\"threshold_usd\"],\"msg\":\"Decimal input should be an integer, float, string or Decimal object\",\"input\":true}]}"}},{"id":"threshold-parsing-array","field":"threshold_usd","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":[]},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"decimal_type\",\"loc\":[\"body\",\"threshold_usd\"],\"msg\":\"Decimal input should be an integer, float, string or Decimal object\",\"input\":[]}]}"}},{"id":"threshold-parsing-object","field":"threshold_usd","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":{}},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"decimal_type\",\"loc\":[\"body\",\"threshold_usd\"],\"msg\":\"Decimal input should be an integer, float, string or Decimal object\",\"input\":{}}]}"}},{"id":"threshold-nan-string","field":"threshold_usd","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"NaN"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"finite_number\",\"loc\":[\"body\",\"threshold_usd\"],\"msg\":\"Input should be a finite number\",\"input\":\"NaN\"}]}"}},{"id":"threshold-infinity-string","field":"threshold_usd","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"Infinity"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"finite_number\",\"loc\":[\"body\",\"threshold_usd\"],\"msg\":\"Input should be a finite number\",\"input\":\"Infinity\"}]}"}},{"id":"threshold-negative-infinity-string","field":"threshold_usd","body":{"product_id":"30000000-0000-4000-8000-000000000001","delivery_point_id":"40000000-0000-4000-8000-000000000001","direction":"above","threshold_usd":"-Infinity"},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"finite_number\",\"loc\":[\"body\",\"threshold_usd\"],\"msg\":\"Input should be a finite number\",\"input\":\"-Infinity\"}]}"}},{"id":"body-array","field":"request","body":[],"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"model_attributes_type\",\"loc\":[\"body\"],\"msg\":\"Input should be a valid dictionary or object to extract fields from\",\"input\":[]}]}"}},{"id":"body-string","field":"request","body":"x","python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"model_attributes_type\",\"loc\":[\"body\"],\"msg\":\"Input should be a valid dictionary or object to extract fields from\",\"input\":\"x\"}]}"}},{"id":"body-number","field":"request","body":1,"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"model_attributes_type\",\"loc\":[\"body\"],\"msg\":\"Input should be a valid dictionary or object to extract fields from\",\"input\":1}]}"}},{"id":"body-boolean","field":"request","body":true,"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"model_attributes_type\",\"loc\":[\"body\"],\"msg\":\"Input should be a valid dictionary or object to extract fields from\",\"input\":true}]}"}},{"id":"body-null","field":"request","body":null,"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"missing\",\"loc\":[\"body\"],\"msg\":\"Field required\",\"input\":null}]}"}},{"id":"multiple-errors","field":"request","body":{"product_id":"bad","direction":"sideways","threshold_usd":0},"python":{"kind":"rejected","status":422,"body":"{\"detail\":[{\"type\":\"uuid_parsing\",\"loc\":[\"body\",\"product_id\"],\"msg\":\"Input should be a valid UUID, invalid length: expected length 32 for simple format, found 3\",\"input\":\"bad\",\"ctx\":{\"error\":\"invalid length: expected length 32 for simple format, found 3\"}},{\"type\":\"value_error\",\"loc\":[\"body\",\"direction\"],\"msg\":\"Value error, direction must be 'above' or 'below'\",\"input\":\"sideways\",\"ctx\":{\"error\":{}}},{\"type\":\"value_error\",\"loc\":[\"body\",\"threshold_usd\"],\"msg\":\"Value error, threshold_usd must be positive\",\"input\":0,\"ctx\":{\"error\":{}}}]}"}}]}"####)
    .unwrap();
    for vector in fixture["vectors"].as_array().unwrap() {
        let result = AlertCreate::validate(vector["body"].clone());
        if vector["python"]["kind"] == "accepted" {
            let parsed = result.unwrap_or_else(|error| panic!("{}: {error:?}", vector["id"]));
            let expected = &vector["python"]["values"];
            assert_eq!(
                parsed.product_id.to_string(),
                expected["product_id"],
                "{}",
                vector["id"]
            );
            assert_eq!(
                json!(parsed.delivery_point_id),
                expected["delivery_point_id"],
                "{}",
                vector["id"]
            );
            assert_eq!(parsed.direction, expected["direction"], "{}", vector["id"]);
        } else {
            let error = result.expect_err(vector["id"].as_str().unwrap());
            let expected: Value =
                serde_json::from_str(vector["python"]["body"].as_str().unwrap()).unwrap();
            assert_eq!(error.status, 422, "{}", vector["id"]);
            assert_eq!(
                json!({"detail": error.detail}),
                expected,
                "{}",
                vector["id"]
            );
        }
    }
}

#[tokio::test]
async fn postgres_crud() {
    use verdaxis_read::crud::alerts::{create, delete, list};
    let Ok(url) = std::env::var("ALERT_TEST_DATABASE_URL") else {
        eprintln!("PostgreSQL component coverage requires rs/read/tests/run_auth_streams_cache.sh");
        return;
    };
    let mut connection = sqlx::PgConnection::connect(&url).await.unwrap();
    let fixture_id = |name| Uuid::parse_str(&std::env::var(name).unwrap()).unwrap();
    let organization = fixture_id("ALERT_TEST_ORGANIZATION");
    let foreign = fixture_id("ALERT_TEST_FOREIGN");
    let product = fixture_id("ALERT_TEST_PRODUCT");
    let point = fixture_id("ALERT_TEST_POINT");
    let policy = verdaxis_read::MarketPolicy {
        source_sha: String::new(),
        products: vec![],
        points: vec![],
    };
    let input = |threshold: &str| {
        AlertCreate::validate(json!({
        "product_id":product,"delivery_point_id":point,"direction":"above","threshold_usd":threshold
    })).unwrap()
    };
    sqlx::query("BEGIN").execute(&mut connection).await.unwrap();
    let first = create(&mut connection, organization, input("12.345"), &policy)
        .await
        .unwrap();
    assert_eq!(first["threshold_usd"], "12.35");
    assert_eq!(first["product_name"], "Alert legacy fixture");
    assert!(first["market_product"].is_null());
    let id = Uuid::parse_str(first["id"].as_str().unwrap()).unwrap();
    assert_eq!(
        delete(&mut connection, foreign, id)
            .await
            .unwrap_err()
            .status,
        403
    );
    assert_eq!(
        list(&mut connection, foreign, &policy).await.unwrap(),
        json!([])
    );
    for _ in 0..4 {
        create(&mut connection, organization, input("1"), &policy)
            .await
            .unwrap();
    }
    assert_eq!(
        create(&mut connection, organization, input("1"), &policy)
            .await
            .unwrap_err()
            .status,
        403
    );
    sqlx::query("UPDATE price_alerts SET is_active=false WHERE id=$1")
        .bind(id)
        .execute(&mut connection)
        .await
        .unwrap();
    let replacement = create(&mut connection, organization, input("0.0001"), &policy)
        .await
        .unwrap();
    assert_eq!(replacement["threshold_usd"], "0.00");
    assert_eq!(
        list(&mut connection, organization, &policy)
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        6
    );
    sqlx::query("DELETE FROM price_alerts WHERE org_id=$1")
        .bind(organization)
        .execute(&mut connection)
        .await
        .unwrap();
    for (threshold, invalid_fk) in [("99999999.995", false), ("1", true)] {
        sqlx::query("SAVEPOINT invalid_alert")
            .execute(&mut connection)
            .await
            .unwrap();
        let mut bad = input(threshold);
        if invalid_fk {
            bad.product_id = Uuid::new_v4();
        }
        assert!(
            create(&mut connection, organization, bad, &policy)
                .await
                .is_err()
        );
        sqlx::query("ROLLBACK TO SAVEPOINT invalid_alert")
            .execute(&mut connection)
            .await
            .unwrap();
    }
    sqlx::query("SAVEPOINT invalid_point")
        .execute(&mut connection)
        .await
        .unwrap();
    let mut bad = input("1");
    bad.delivery_point_id = Some(Uuid::new_v4());
    assert!(
        create(&mut connection, organization, bad, &policy)
            .await
            .is_err()
    );
    sqlx::query("ROLLBACK TO SAVEPOINT invalid_point")
        .execute(&mut connection)
        .await
        .unwrap();
    let final_alert = create(&mut connection, organization, input("12.345"), &policy)
        .await
        .unwrap();
    let removable = create(&mut connection, organization, input("1"), &policy)
        .await
        .unwrap();
    let removable = Uuid::parse_str(removable["id"].as_str().unwrap()).unwrap();
    delete(&mut connection, organization, removable)
        .await
        .unwrap();
    assert_eq!(
        delete(&mut connection, organization, removable)
            .await
            .unwrap_err()
            .status,
        404
    );
    assert_ne!(first["id"], final_alert["id"]);
    sqlx::query("COMMIT")
        .execute(&mut connection)
        .await
        .unwrap();
    postgres_http_admission(&url, organization, product, point).await;
}

async fn postgres_http_admission(url: &str, organization: Uuid, product: Uuid, point: Uuid) {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    use verdaxis_read::auth::{VerificationSettings, limits::SharedLimits};
    use verdaxis_read::{AppState, Config, MarketPolicy};
    let config = Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: ALLOWED_REVISIONS[0].into(),
        database_url: url.into(),
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
        phase2: Default::default(),
    };
    let pools = verdaxis_read::connect(&config).await.unwrap();
    let state = AppState::new(
        config,
        pools,
        MarketPolicy {
            source_sha: String::new(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap();
    let key = "synthetic-alert-http-key-minimum-32-characters";
    let app = verdaxis_read::crud::alerts::router(
        state,
        VerificationSettings::new(key.into(), None, "alert-test".into(), "alert-test".into()),
        SharedLimits::default(),
    );
    let actors: Vec<String> = std::env::var("ALERT_TEST_ACTORS")
        .unwrap()
        .split(',')
        .map(str::to_owned)
        .collect();
    let token = |actor: &str| {
        let now = chrono::Utc::now().timestamp();
        jsonwebtoken::encode(&jsonwebtoken::Header::default(), &json!({
            "sub":actor,"type":"access","iss":"alert-test","aud":"alert-test","iat":now,"exp":now+300
        }), &jsonwebtoken::EncodingKey::from_secret(key.as_bytes())).unwrap()
    };
    let existing = sqlx::query_scalar::<_, Uuid>("SELECT id FROM price_alerts WHERE org_id=$1")
        .bind(organization)
        .fetch_one(&mut sqlx::PgConnection::connect(url).await.unwrap())
        .await
        .unwrap();
    for (method, path, actor, payload, status) in [
        (
            "POST",
            "/api/alerts".to_owned(),
            3,
            json!({"product_id":product,"delivery_point_id":point,"direction":"above","threshold_usd":"1"}),
            400,
        ),
        ("GET", "/api/alerts".to_owned(), 3, Value::Null, 400),
        (
            "DELETE",
            format!("/api/alerts/{existing}"),
            3,
            Value::Null,
            400,
        ),
        (
            "DELETE",
            format!("/api/alerts/{existing}"),
            2,
            Value::Null,
            403,
        ),
        ("DELETE", "/api/alerts/bad".to_owned(), 0, Value::Null, 422),
        ("GET", "/api/alerts".to_owned(), 1, Value::Null, 200),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("authorization", format!("Bearer {}", token(&actors[actor])))
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        if method == "GET" && status == 200 {
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap())
                    .unwrap();
            assert_eq!(body.as_array().unwrap().len(), 1);
        }
    }
}

#[test]
fn positive_threshold_does_not_narrow_before_postgres_rounding() {
    for threshold in [
        "0.0001",
        "99999999.995",
        "123456789012345678901234567890.123",
        "1e100",
    ] {
        let parsed = AlertCreate::validate(json!({
            "product_id": Uuid::from_u128(1), "direction": "below", "threshold_usd": threshold
        }))
        .unwrap();
        assert!(!parsed.threshold_usd.is_empty());
    }
}
