use serde_json::json;
use verdaxis_read::routes::prices::reference_csv;

#[test]
fn reference_csv_matches_python_quoting_and_crlf() {
    let rows = [
        json!({"date":"2026-10-15","product_name":"A, \"quoted\"\nname", "market_product":"FIXTURE", "availability_window":"SPOT", "fuel_type":"type", "delivery_point_name":null, "region":"region", "vwap_usd":"12.50", "total_volume_mt":"2.000", "trade_count":2}),
    ];
    assert_eq!(
        String::from_utf8(reference_csv(&rows).unwrap()).unwrap(),
        "date,product_name,market_product,availability_window,fuel_type,delivery_point_name,region,vwap_usd,volume_mt,trade_count\r\n2026-10-15,\"A, \"\"quoted\"\"\nname\",FIXTURE,SPOT,type,,region,12.50,2.000,2\r\n"
    );
}

#[test]
fn price_rounding_uses_half_even() {
    use verdaxis_read::routes::prices::rounded_price;
    for (input, expected) in [
        ("12.505", "12.50"),
        ("12.515", "12.52"),
        ("-12.505", "-12.50"),
        ("0", "0.00"),
        ("1.2", "1.20"),
    ] {
        assert_eq!(rounded_price(input).unwrap(), expected);
    }
}

#[test]
fn empty_reference_csv_has_header_only() {
    assert_eq!(reference_csv(&[]).unwrap(), b"date,product_name,market_product,availability_window,fuel_type,delivery_point_name,region,vwap_usd,volume_mt,trade_count\r\n");
}
