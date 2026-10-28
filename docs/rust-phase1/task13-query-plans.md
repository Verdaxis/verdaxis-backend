# Task 13 actual query plans — projection checkpoint

Plans were captured with auto_explain ANALYZE, BUFFERS, timing and JSON on the same native disposable database per run. This records actual bound SQL, not a reconstructed approximation. Timed plan runs are diagnostic, not the 240-request acceptance measurement.

| Route | Statement | Python before ms | Rust before ms | Python after ms | Rust after ms |
| --- | --- | ---: | ---: | ---: | ---: |
| /api/orderbook | page | 149.861 | 1578.248 | 211.995 | 387.519 |
| /api/orderbook | benchmark | 128.604 | 3465.408 | 182.099 | 728.775 |
| /api/orderbook/bids | count | 102.813 | 264.909 | 167.765 | 321.397 |
| /api/orderbook/bids | page | 161.561 | 1772.503 | 255.572 | 337.868 |
| /api/orderbook/bids | benchmark | 145.067 | 2629.365 | 180.715 | 837.731 |

## Attribution

Rust page projection ran below the sort for about 10,000 rows. The nested-loop subtree took 1.54–1.73 seconds, while the sort added about 30 ms. The new narrow page-key CTE limits before lateral detail projection. The shared eligible relation is NOT MATERIALIZED so the repeated key lookup does not force full private projection.

Rust benchmark projected full private JSON for roughly 10,047 candidates. It now constructs only the fields consumed by the unchanged benchmark qualifier. Economic/typed predicates, exact Decimal strings and captured Singapore date are unchanged. Repeated policy/lane subplans remain; this is a partial repair, not p95 acceptance.

## Exact SQL and plans (before)

### python /api/orderbook rows=50

```sql
SELECT products.id, products.name, products.fuel_type, products.fuel_grade, products.unit, products.min_lot_size, products.spec_description, products.is_active, products.created_at, delivery_points.id AS id_1, delivery_points.name AS name_1, delivery_points.region, delivery_points.timezone, delivery_points.is_active AS is_active_1, delivery_points.created_at AS created_at_1, orderbook_orders.id AS id_2, orderbook_orders.acceptance_ordinal, orderbook_orders.organization_id, orderbook_orders.owner_user_id, orderbook_orders.created_by_actor_user_id, orderbook_orders.creation_method, orderbook_orders.support_authorization_id, orderbook_orders.version, orderbook_orders.inventory_item_id, orderbook_orders.provenance, orderbook_orders.side, orderbook_orders.product_id, orderbook_orders.delivery_point_id, orderbook_orders.port_id, orderbook_orders.vessel_id, orderbook_orders.quantity_mt, orderbook_orders.remaining_quantity_mt, orderbook_orders.price_per_mt_usd, orderbook_orders.availability_window, orderbook_orders.delivery_window_start, orderbook_orders.delivery_window_end, orderbook_orders.certifications, orderbook_orders.certification_declared, orderbook_orders.certification_scheme, orderbook_orders.fame_terms, orderbook_orders.lng_terms, orderbook_orders.specification_standard, orderbook_orders.msds_available, orderbook_orders.is_verdaxis_verified, orderbook_orders.carbon_intensity_gco2_mj, orderbook_orders.carbon_intensity_method, orderbook_orders.energy_density_mj_kg, orderbook_orders.feedstock, orderbook_orders.origin, orderbook_orders.off_spec, orderbook_orders.off_spec_notes, orderbook_orders.status, orderbook_orders.expires_at, orderbook_orders.idempotency_key, orderbook_orders.idempotency_operation, orderbook_orders.idempotency_request_hash, orderbook_orders.created_at AS created_at_2, orderbook_orders.updated_at, organizations_1.id AS id_3, organizations_1.name AS name_2, organizations_1.domain, organizations_1.type, organizations_1.supplier_tier, organizations_1.tax_id, organizations_1.country_code, organizations_1.verification_status, organizations_1.provenance AS provenance_1, organizations_1.created_at AS created_at_3 
FROM orderbook_orders JOIN products ON orderbook_orders.product_id = products.id JOIN delivery_points ON orderbook_orders.delivery_point_id = delivery_points.id LEFT OUTER JOIN organizations AS organizations_1 ON organizations_1.id = orderbook_orders.organization_id 
WHERE orderbook_orders.status IN ($132, $133) AND products.is_active IS true AND (products.id = $1::UUID AND products.name = $2::VARCHAR AND products.fuel_type = $3::VARCHAR AND products.fuel_grade = $4::VARCHAR OR products.id = $5::UUID AND products.name = $6::VARCHAR AND products.fuel_type = $7::VARCHAR AND products.fuel_grade = $8::VARCHAR OR products.id = $9::UUID AND products.name = $10::VARCHAR AND products.fuel_type = $11::VARCHAR AND products.fuel_grade = $12::VARCHAR OR products.id = $13::UUID AND products.name = $14::VARCHAR AND products.fuel_type = $15::VARCHAR AND products.fuel_grade = $16::VARCHAR OR products.id = $17::UUID AND products.name = $18::VARCHAR AND products.fuel_type = $19::VARCHAR AND products.fuel_grade = $20::VARCHAR OR products.id = $21::UUID AND products.name = $22::VARCHAR AND products.fuel_type = $23::VARCHAR AND products.fuel_grade = $24::VARCHAR) AND CASE WHEN (products.id = $25::UUID AND products.name = $26::VARCHAR AND products.fuel_type = $27::VARCHAR AND products.fuel_grade = $28::VARCHAR) THEN $29::VARCHAR WHEN (products.id = $30::UUID AND products.name = $31::VARCHAR AND products.fuel_type = $32::VARCHAR AND products.fuel_grade = $33::VARCHAR) THEN $34::VARCHAR WHEN (products.id = $35::UUID AND products.name = $36::VARCHAR AND products.fuel_type = $37::VARCHAR AND products.fuel_grade = $38::VARCHAR) THEN $39::VARCHAR WHEN (products.id = $40::UUID AND products.name = $41::VARCHAR AND products.fuel_type = $42::VARCHAR AND products.fuel_grade = $43::VARCHAR) THEN $44::VARCHAR WHEN (products.id = $45::UUID AND products.name = $46::VARCHAR AND products.fuel_type = $47::VARCHAR AND products.fuel_grade = $48::VARCHAR) THEN $49::VARCHAR WHEN (products.id = $50::UUID AND products.name = $51::VARCHAR AND products.fuel_type = $52::VARCHAR AND products.fuel_grade = $53::VARCHAR) THEN $54::VARCHAR END IN ($134::VARCHAR, $135::VARCHAR, $136::VARCHAR, $137::VARCHAR, $138::VARCHAR, $139::VARCHAR) AND delivery_points.is_active IS true AND (delivery_points.id = $55::UUID AND delivery_points.name = $56::VARCHAR AND delivery_points.region = $57::VARCHAR OR delivery_points.id = $58::UUID AND delivery_points.name = $59::VARCHAR AND delivery_points.region = $60::VARCHAR OR delivery_points.id = $61::UUID AND delivery_points.name = $62::VARCHAR AND delivery_points.region = $63::VARCHAR OR delivery_points.id = $64::UUID AND delivery_points.name = $65::VARCHAR AND delivery_points.region = $66::VARCHAR OR delivery_points.id = $67::UUID AND delivery_points.name = $68::VARCHAR AND delivery_points.region = $69::VARCHAR OR delivery_points.id = $70::UUID AND delivery_points.name = $71::VARCHAR AND delivery_points.region = $72::VARCHAR OR delivery_points.id = $73::UUID AND delivery_points.name = $74::VARCHAR AND delivery_points.region = $75::VARCHAR OR delivery_points.id = $76::UUID AND delivery_points.name = $77::VARCHAR AND delivery_points.region = $78::VARCHAR) AND (products.id = $79::UUID AND delivery_points.id IN ($140::UUID) OR products.id = $80::UUID AND delivery_points.id IN ($141::UUID) OR products.id = $81::UUID OR products.id = $82::UUID OR products.id = $83::UUID OR products.id = $84::UUID) AND (orderbook_orders.availability_window = $85::VARCHAR OR orderbook_orders.availability_window ~ $86::VARCHAR OR orderbook_orders.availability_window ~ $87::VARCHAR OR orderbook_orders.availability_window ~ $88::VARCHAR) AND (orderbook_orders.expires_at > $89::TIMESTAMP WITH TIME ZONE OR orderbook_orders.expires_at IS NULL AND orderbook_orders.provenance IN ($142, $143)) AND CASE WHEN (orderbook_orders.creation_method = $90) THEN EXISTS (SELECT users.id 
FROM users 
WHERE orderbook_orders.creation_method = $91 AND users.id = orderbook_orders.owner_user_id AND users.id = orderbook_orders.created_by_actor_user_id AND users.role = $92 AND users.status = $93 AND (EXISTS (SELECT users_1.id 
FROM users AS users_1 
WHERE users_1.organization_id = orderbook_orders.organization_id AND users_1.role IN ($144, $145) AND users_1.status = $94 AND users_1.email_verified IS true AND users_1.must_change_password IS false AND users_1.kyc_status != $95::VARCHAR AND (users_1.kyc_status = $96::VARCHAR AND users_1.kyc_organization_id = users_1.organization_id OR (EXISTS (SELECT kyc_execution_grace.id 
FROM kyc_execution_grace 
WHERE kyc_execution_grace.user_id = users_1.id AND kyc_execution_grace.organization_id = users_1.organization_id AND kyc_execution_grace.expires_at > $97::TIMESTAMP WITH TIME ZONE))))) AND (EXISTS (SELECT organizations.id 
FROM organizations 
WHERE organizations.id = orderbook_orders.organization_id AND organizations.verification_status = $98::VARCHAR AND organizations.provenance = $99))) ELSE orderbook_orders.owner_user_id IS NULL OR (EXISTS (SELECT users.id 
FROM users 
WHERE users.id = orderbook_orders.owner_user_id AND users.organization_id = orderbook_orders.organization_id AND users.role IN ($146, $147) AND users.status = $100 AND users.email_verified IS true AND users.must_change_password IS false AND users.kyc_status != $101::VARCHAR AND (users.kyc_status = $102::VARCHAR AND users.kyc_organization_id = users.organization_id OR (EXISTS (SELECT kyc_execution_grace.id 
FROM kyc_execution_grace 
WHERE kyc_execution_grace.user_id = users.id AND kyc_execution_grace.organization_id = users.organization_id AND kyc_execution_grace.expires_at > $103::TIMESTAMP WITH TIME ZONE))) AND (EXISTS (SELECT organizations.id 
FROM organizations 
WHERE organizations.id = orderbook_orders.organization_id AND organizations.verification_status = $104::VARCHAR)))) END AND (orderbook_orders.product_id != $105::UUID OR orderbook_orders.delivery_point_id IN ($148::UUID) AND CAST((orderbook_orders.fame_terms ->> $106::TEXT) AS VARCHAR) = CAST(orderbook_orders.side AS VARCHAR) AND orderbook_orders.off_spec IS false AND (orderbook_orders.side != $107 OR CAST((orderbook_orders.fame_terms ->> $108::TEXT) AS VARCHAR) >= $109::VARCHAR AND orderbook_orders.msds_available IS true AND orderbook_orders.certification_declared IS true AND length(trim(coalesce(orderbook_orders.certification_scheme, $110::VARCHAR))) > $111::INTEGER)) AND (orderbook_orders.product_id != $112::UUID OR orderbook_orders.delivery_point_id IN ($149::UUID) AND CAST((orderbook_orders.lng_terms ->> $113::TEXT) AS VARCHAR) = CAST(orderbook_orders.side AS VARCHAR) AND orderbook_orders.off_spec IS false) AND orderbook_orders.provenance IN ($150, $151) AND products.fuel_type IN ($152::VARCHAR, $153::VARCHAR, $154::VARCHAR, $155::VARCHAR) AND orderbook_orders.off_spec IS false AND (orderbook_orders.side != $114 OR length(trim(coalesce(orderbook_orders.certification_scheme, $115::VARCHAR))) > $116::INTEGER) AND (orderbook_orders.side != $117 OR orderbook_orders.certification_declared IS true) AND (orderbook_orders.side != $118 OR length(trim(coalesce(orderbook_orders.specification_standard, $119::VARCHAR))) > $120::INTEGER) AND (orderbook_orders.side != $121 OR orderbook_orders.msds_available IS true) AND (orderbook_orders.side != $122 OR orderbook_orders.product_id IN ($156::UUID, $157::UUID) OR orderbook_orders.carbon_intensity_gco2_mj IS NOT NULL) AND (orderbook_orders.side != $123 OR length(trim(coalesce(orderbook_orders.feedstock, $124::VARCHAR))) > $125::INTEGER) AND (orderbook_orders.side != $126 OR orderbook_orders.product_id = $127::UUID OR length(trim(coalesce(orderbook_orders.origin, $128::VARCHAR))) > $129::INTEGER) ORDER BY orderbook_orders.created_at DESC, orderbook_orders.id DESC 
 LIMIT $130::INTEGER OFFSET $131::INTEGER
```

```json
{
  "Node Type": "Limit",
  "Parallel Aware": false,
  "Async Capable": false,
  "Startup Cost": 13448.44,
  "Total Cost": 13448.57,
  "Plan Rows": 50,
  "Plan Width": 3350,
  "Actual Startup Time": 149.834,
  "Actual Total Time": 149.861,
  "Actual Rows": 50,
  "Actual Loops": 1,
  "Shared Hit Blocks": 30529,
  "Shared Read Blocks": 0,
  "Shared Dirtied Blocks": 0,
  "Shared Written Blocks": 0,
  "Local Hit Blocks": 0,
  "Local Read Blocks": 0,
  "Local Dirtied Blocks": 0,
  "Local Written Blocks": 0,
  "Temp Read Blocks": 0,
  "Temp Written Blocks": 0,
  "Plans": [
    {
      "Node Type": "Sort",
      "Parent Relationship": "Outer",
      "Parallel Aware": false,
      "Async Capable": false,
      "Startup Cost": 13448.44,
      "Total Cost": 13448.6,
      "Plan Rows": 62,
      "Plan Width": 3350,
      "Actual Startup Time": 149.832,
      "Actual Total Time": 149.851,
      "Actual Rows": 50,
      "Actual Loops": 1,
      "Sort Key": [
        "orderbook_orders.created_at DESC",
        "orderbook_orders.id DESC"
      ],
      "Sort Method": "top-N heapsort",
      "Sort Space Used": 69,
      "Sort Space Type": "Memory",
      "Shared Hit Blocks": 30529,
      "Shared Read Blocks": 0,
      "Shared Dirtied Blocks": 0,
      "Shared Written Blocks": 0,
      "Local Hit Blocks": 0,
      "Local Read Blocks": 0,
      "Local Dirtied Blocks": 0,
      "Local Written Blocks": 0,
      "Temp Read Blocks": 0,
      "Temp Written Blocks": 0,
      "Plans": [
        {
          "Node Type": "Nested Loop",
          "Parent Relationship": "Outer",
          "Parallel Aware": false,
          "Async Capable": false,
          "Join Type": "Left",
          "Startup Cost": 10.65,
          "Total Cost": 13446.6,
          "Plan Rows": 62,
          "Plan Width": 3350,
          "Actual Startup Time": 0.992,
          "Actual Total Time": 127.224,
          "Actual Rows": 10045,
          "Actual Loops": 1,
          "Inner Unique": true,
          "Join Filter": "(organizations_1.id = orderbook_orders.organization_id)",
          "Rows Removed by Join Filter": 30153,
          "Shared Hit Blocks": 30523,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0,
          "Plans": [
            {
              "Node Type": "Nested Loop",
              "Parent Relationship": "Outer",
              "Parallel Aware": false,
              "Async Capable": false,
              "Join Type": "Inner",
              "Startup Cost": 10.65,
              "Total Cost": 13434.18,
              "Plan Rows": 62,
              "Plan Width": 3087,
              "Actual Startup Time": 0.973,
              "Actual Total Time": 112.308,
              "Actual Rows": 10045,
              "Actual Loops": 1,
              "Inner Unique": false,
              "Shared Hit Blocks": 30522,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0,
              "Plans": [
                {
                  "Node Type": "Nested Loop",
                  "Parent Relationship": "Outer",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Join Type": "Inner",
                  "Startup Cost": 0.0,
                  "Total Cost": 3.76,
                  "Plan Rows": 1,
                  "Plan Width": 204,
                  "Actual Startup Time": 0.062,
                  "Actual Total Time": 0.125,
                  "Actual Rows": 23,
                  "Actual Loops": 1,
                  "Inner Unique": false,
                  "Join Filter": "(((products.id = '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid) AND (delivery_points.id = '1379d36c-1ca9-55b7-9c0d-5235a0ba1f36'::uuid)) OR ((products.id = 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) AND (delivery_points.id = '73835e92-820e-584b-8280-bb61c63aa28e'::uuid)) OR (products.id = '9510c713-6e39-5080-add3-0a7c29657b79'::uuid) OR (products.id = 'f9b20492-b445-59cd-b292-a386d913f488'::uuid) OR (products.id = 'c4a688be-f7c2-5edc-8f93-6b34e387609c'::uuid) OR (products.id = 'd186bffb-766d-5944-8825-989abbdcfc46'::uuid))",
                  "Rows Removed by Join Filter": 12,
                  "Shared Hit Blocks": 6,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0,
                  "Plans": [
                    {
                      "Node Type": "Seq Scan",
                      "Parent Relationship": "Outer",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Relation Name": "products",
                      "Alias": "products",
                      "Startup Cost": 0.0,
                      "Total Cost": 2.1,
                      "Plan Rows": 1,
                      "Plan Width": 149,
                      "Actual Startup Time": 0.035,
                      "Actual Total Time": 0.05,
                      "Actual Rows": 5,
                      "Actual Loops": 1,
                      "Filter": "((is_active IS TRUE) AND ((fuel_type)::text = ANY ('{Methanol,Ethanol,FAME,LNG}'::text[])) AND ((id = '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid) OR (id = 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) OR (id = '9510c713-6e39-5080-add3-0a7c29657b79'::uuid) OR (id = 'f9b20492-b445-59cd-b292-a386d913f488'::uuid) OR (id = 'c4a688be-f7c2-5edc-8f93-6b34e387609c'::uuid) OR (id = 'd186bffb-766d-5944-8825-989abbdcfc46'::uuid)) AND (((id = '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid) AND ((name)::text = 'Bio-LNG'::text) AND ((fuel_type)::text = 'LNG'::text) AND ((fuel_grade)::text = 'Bio'::text)) OR ((id = 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) AND ((name)::text = 'UCOME B100'::text) AND ((fuel_type)::text = 'FAME'::text) AND ((fuel_grade)::text = 'UCOME'::text)) OR ((id = '9510c713-6e39-5080-add3-0a7c29657b79'::uuid) AND ((name)::text = 'Bio Methanol'::text) AND ((fuel_type)::text = 'Methanol'::text) AND ((fuel_grade)::text = 'Bio'::text)) OR ((id = 'f9b20492-b445-59cd-b292-a386d913f488'::uuid) AND ((name)::text = 'e-Methanol'::text) AND ((fuel_type)::text = 'Methanol'::text) AND ((fuel_grade)::text = 'E'::text)) OR ((id = 'c4a688be-f7c2-5edc-8f93-6b34e387609c'::uuid) AND ((name)::text = 'Bio Ethanol'::text) AND ((fuel_type)::text = 'Ethanol'::text) AND ((fuel_grade)::text = 'Bio'::text)) OR ((id = 'd186bffb-766d-5944-8825-989abbdcfc46'::uuid) AND ((name)::text = 'Synthetic Ethanol'::text) AND ((fuel_type)::text = 'Ethanol'::text) AND ((fuel_grade)::text = 'Synthetic'::text))) AND ((CASE WHEN ((id = '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid) AND ((name)::text = 'Bio-LNG'::text) AND ((fuel_type)::text = 'LNG'::text) AND ((fuel_grade)::text = 'Bio'::text)) THEN 'BIO_LNG'::character varying WHEN ((id = 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) AND ((name)::text = 'UCOME B100'::text) AND ((fuel_type)::text = 'FAME'::text) AND ((fuel_grade)::text = 'UCOME'::text)) THEN 'UCOME_B100'::character varying WHEN ((id = '9510c713-6e39-5080-add3-0a7c29657b79'::uuid) AND ((name)::text = 'Bio Methanol'::text) AND ((fuel_type)::text = 'Methanol'::text) AND ((fuel_grade)::text = 'Bio'::text)) THEN 'BIO_METHANOL'::character varying WHEN ((id = 'f9b20492-b445-59cd-b292-a386d913f488'::uuid) AND ((name)::text = 'e-Methanol'::text) AND ((fuel_type)::text = 'Methanol'::text) AND ((fuel_grade)::text = 'E'::text)) THEN 'E_METHANOL'::character varying WHEN ((id = 'c4a688be-f7c2-5edc-8f93-6b34e387609c'::uuid) AND ((name)::text = 'Bio Ethanol'::text) AND ((fuel_type)::text = 'Ethanol'::text) AND ((fuel_grade)::text = 'Bio'::text)) THEN 'BIO_ETHANOL'::character varying WHEN ((id = 'd186bffb-766d-5944-8825-989abbdcfc46'::uuid) AND ((name)::text = 'Synthetic Ethanol'::text) AND ((fuel_type)::text = 'Ethanol'::text) AND ((fuel_grade)::text = 'Synthetic'::text)) THEN 'SYNTHETIC_ETHANOL'::character varying ELSE NULL::character varying END)::text = ANY ('{BIO_LNG,UCOME_B100,BIO_METHANOL,E_METHANOL,BIO_ETHANOL,SYNTHETIC_ETHANOL}'::text[])))",
                      "Rows Removed by Filter": 2,
                      "Shared Hit Blocks": 1,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0
                    },
                    {
                      "Node Type": "Seq Scan",
                      "Parent Relationship": "Inner",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Relation Name": "delivery_points",
                      "Alias": "delivery_points",
                      "Startup Cost": 0.0,
                      "Total Cost": 1.63,
                      "Plan Rows": 1,
                      "Plan Width": 55,
                      "Actual Startup Time": 0.006,
                      "Actual Total Time": 0.01,
                      "Actual Rows": 7,
                      "Actual Loops": 5,
                      "Filter": "((is_active IS TRUE) AND (((id = '78281fb3-e726-5396-802e-34b01fda21e8'::uuid) AND ((name)::text = 'Dalian'::text) AND ((region)::text = 'Asia'::text)) OR ((id = '262d36ae-6f35-5785-b9e8-9e221b0f1b78'::uuid) AND ((name)::text = 'Busan'::text) AND ((region)::text = 'Asia'::text)) OR ((id = '633c0593-f9b9-52ad-abe5-a0285f6888d3'::uuid) AND ((name)::text = 'Shanghai'::text) AND ((region)::text = 'Asia'::text)) OR ((id = '73835e92-820e-584b-8280-bb61c63aa28e'::uuid) AND ((name)::text = 'Singapore'::text) AND ((region)::text = 'Asia'::text)) OR ((id = '1379d36c-1ca9-55b7-9c0d-5235a0ba1f36'::uuid) AND ((name)::text = 'Rotterdam'::text) AND ((region)::text = 'Europe'::text)) OR ((id = 'a083db06-b050-56c2-a274-3897eac2fdae'::uuid) AND ((name)::text = 'Houston'::text) AND ((region)::text = 'Americas'::text)) OR ((id = '1be2a5cb-fc34-5e88-8b15-7f7d0fd6e534'::uuid) AND ((name)::text = 'Los Angeles'::text) AND ((region)::text = 'Americas'::text)) OR ((id = 'a4762444-4e0b-5bc1-b647-a185047adae5'::uuid) AND ((name)::text = 'Santos'::text) AND ((region)::text = 'Americas'::text))))",
                      "Rows Removed by Filter": 2,
                      "Shared Hit Blocks": 5,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0
                    }
                  ]
                },
                {
                  "Node Type": "Bitmap Heap Scan",
                  "Parent Relationship": "Inner",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Relation Name": "orderbook_orders",
                  "Alias": "orderbook_orders",
                  "Startup Cost": 10.65,
                  "Total Cost": 13427.96,
                  "Plan Rows": 246,
                  "Plan Width": 2883,
                  "Actual Startup Time": 0.048,
                  "Actual Total Time": 4.692,
                  "Actual Rows": 437,
                  "Actual Loops": 23,
                  "Recheck Cond": "((product_id = products.id) AND (delivery_point_id = delivery_points.id) AND ((status)::text = ANY ('{OPEN,PARTIALLY_FILLED}'::text[])))",
                  "Rows Removed by Index Recheck": 0,
                  "Filter": "((off_spec IS FALSE) AND ((provenance)::text = ANY ('{REAL,DEMO}'::text[])) AND (((side)::text <> 'ASK'::text) OR (certification_declared IS TRUE)) AND (((side)::text <> 'ASK'::text) OR (msds_available IS TRUE)) AND ((expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone) OR ((expires_at IS NULL) AND ((provenance)::text = ANY ('{REAL,UNKNOWN}'::text[])))) AND (((side)::text <> 'ASK'::text) OR (product_id = ANY ('{e561e43f-d9b2-598e-981c-f1d28d515ddc,8f6f34a5-2927-54a6-8564-cca40ae7cb8a}'::uuid[])) OR (carbon_intensity_gco2_mj IS NOT NULL)) AND (((availability_window)::text = 'SPOT'::text) OR ((availability_window)::text ~ '^[0-9]{4}-(0[1-9]|1[0-2])$'::text) OR ((availability_window)::text ~ '^[0-9]{4}-Q[1-4]$'::text) OR ((availability_window)::text ~ '^[0-9]{4}-CAL$'::text)) AND ((product_id <> '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid) OR ((delivery_point_id = '1379d36c-1ca9-55b7-9c0d-5235a0ba1f36'::uuid) AND ((lng_terms ->> 'side'::text) = (side)::text) AND (off_spec IS FALSE))) AND (((side)::text <> 'ASK'::text) OR (length(TRIM(BOTH FROM COALESCE(certification_scheme, ''::character varying))) > 0)) AND (((side)::text <> 'ASK'::text) OR (length(TRIM(BOTH FROM COALESCE(specification_standard, ''::character varying))) > 0)) AND (((side)::text <> 'ASK'::text) OR (length(TRIM(BOTH FROM COALESCE(feedstock, ''::character varying))) > 0)) AND (((side)::text <> 'ASK'::text) OR (product_id = 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) OR (length(TRIM(BOTH FROM COALESCE(origin, ''::character varying))) > 0)) AND ((product_id <> 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) OR ((delivery_point_id = '73835e92-820e-584b-8280-bb61c63aa28e'::uuid) AND ((fame_terms ->> 'side'::text) = (side)::text) AND (off_spec IS FALSE) AND (((side)::text <> 'ASK'::text) OR (((fame_terms ->> 'certificate_valid_until'::text) >= '2026-10-15'::text) AND (msds_available IS TRUE) AND (certification_declared IS TRUE) AND (length(TRIM(BOTH FROM COALESCE(certification_scheme, ''::character varying))) > 0))))) AND CASE WHEN ((creation_method)::text = 'MARKET_SUPPORT'::text) THEN EXISTS(SubPlan 5) ELSE ((owner_user_id IS NULL) OR EXISTS(SubPlan 9)) END)",
                  "Rows Removed by Filter": 1,
                  "Exact Heap Blocks": 277,
                  "Lossy Heap Blocks": 0,
                  "Shared Hit Blocks": 30516,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0,
                  "Plans": [
                    {
                      "Node Type": "Bitmap Index Scan",
                      "Parent Relationship": "Outer",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Index Name": "ix_orderbook_orders_active_slice_expiry",
                      "Startup Cost": 0.0,
                      "Total Cost": 10.59,
                      "Plan Rows": 504,
                      "Plan Width": 0,
                      "Actual Startup Time": 0.035,
                      "Actual Total Time": 0.035,
                      "Actual Rows": 440,
                      "Actual Loops": 23,
                      "Index Cond": "((product_id = products.id) AND (delivery_point_id = delivery_points.id) AND ((status)::text = ANY ('{OPEN,PARTIALLY_FILLED}'::text[])))",
                      "Shared Hit Blocks": 57,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0
                    },
                    {
                      "Node Type": "Result",
                      "Parent Relationship": "SubPlan",
                      "Subplan Name": "SubPlan 5",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Startup Cost": 14.05,
                      "Total Cost": 15.45,
                      "Plan Rows": 1,
                      "Plan Width": 0,
                      "Actual Startup Time": 0.015,
                      "Actual Total Time": 0.015,
                      "Actual Rows": 0,
                      "Actual Loops": 7,
                      "One-Time Filter": "(((orderbook_orders.creation_method)::text = 'MARKET_SUPPORT'::text) AND (InitPlan 3).col1 AND (InitPlan 4).col1 AND (orderbook_orders.owner_user_id = orderbook_orders.created_by_actor_user_id))",
                      "Shared Hit Blocks": 22,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0,
                      "Plans": [
                        {
                          "Node Type": "Index Scan",
                          "Parent Relationship": "InitPlan",
                          "Subplan Name": "InitPlan 3",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Scan Direction": "Forward",
                          "Index Name": "ix_users_organization_id_id",
                          "Relation Name": "users",
                          "Alias": "users_1",
                          "Startup Cost": 0.14,
                          "Total Cost": 12.82,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.007,
                          "Actual Total Time": 0.007,
                          "Actual Rows": 1,
                          "Actual Loops": 7,
                          "Index Cond": "(organization_id = orderbook_orders.organization_id)",
                          "Rows Removed by Index Recheck": 0,
                          "Filter": "((email_verified IS TRUE) AND (must_change_password IS FALSE) AND ((role)::text = ANY ('{BUYER,SUPPLIER}'::text[])) AND ((kyc_status)::text <> 'REJECTED'::text) AND ((status)::text = 'APPROVED'::text) AND ((((kyc_status)::text = 'APPROVED'::text) AND (kyc_organization_id = organization_id)) OR (ANY ((id = (hashed SubPlan 2).col1) AND (organization_id = (hashed SubPlan 2).col2)))))",
                          "Rows Removed by Filter": 0,
                          "Shared Hit Blocks": 15,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0,
                          "Plans": [
                            {
                              "Node Type": "Seq Scan",
                              "Parent Relationship": "SubPlan",
                              "Subplan Name": "SubPlan 2",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Relation Name": "kyc_execution_grace",
                              "Alias": "kyc_execution_grace",
                              "Startup Cost": 0.0,
                              "Total Cost": 1.1,
                              "Plan Rows": 6,
                              "Plan Width": 32,
                              "Actual Startup Time": 0.003,
                              "Actual Total Time": 0.004,
                              "Actual Rows": 6,
                              "Actual Loops": 1,
                              "Filter": "(expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone)",
                              "Rows Removed by Filter": 2,
                              "Shared Hit Blocks": 1,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            }
                          ]
                        },
                        {
                          "Node Type": "Seq Scan",
                          "Parent Relationship": "InitPlan",
                          "Subplan Name": "InitPlan 4",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Relation Name": "organizations",
                          "Alias": "organizations",
                          "Startup Cost": 0.0,
                          "Total Cost": 1.23,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.002,
                          "Actual Total Time": 0.003,
                          "Actual Rows": 1,
                          "Actual Loops": 4,
                          "Filter": "((id = orderbook_orders.organization_id) AND ((verification_status)::text = 'APPROVED'::text) AND ((provenance)::text = 'REAL'::text))",
                          "Rows Removed by Filter": 4,
                          "Shared Hit Blocks": 4,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0
                        },
                        {
                          "Node Type": "Seq Scan",
                          "Parent Relationship": "Outer",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Relation Name": "users",
                          "Alias": "users",
                          "Startup Cost": 14.05,
                          "Total Cost": 15.45,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.01,
                          "Actual Total Time": 0.01,
                          "Actual Rows": 1,
                          "Actual Loops": 3,
                          "Filter": "((id = orderbook_orders.created_by_actor_user_id) AND ((role)::text = 'ADMIN'::text) AND ((status)::text = 'APPROVED'::text))",
                          "Rows Removed by Filter": 13,
                          "Shared Hit Blocks": 3,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0
                        }
                      ]
                    },
                    {
                      "Node Type": "Result",
                      "Parent Relationship": "SubPlan",
                      "Subplan Name": "SubPlan 9",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Startup Cost": 1.33,
                      "Total Cost": 10.5,
                      "Plan Rows": 1,
                      "Plan Width": 0,
                      "Actual Startup Time": 0.007,
                      "Actual Total Time": 0.007,
                      "Actual Rows": 1,
                      "Actual Loops": 10054,
                      "One-Time Filter": "(InitPlan 8).col1",
                      "Shared Hit Blocks": 30160,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0,
                      "Plans": [
                        {
                          "Node Type": "Seq Scan",
                          "Parent Relationship": "InitPlan",
                          "Subplan Name": "InitPlan 8",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Relation Name": "organizations",
                          "Alias": "organizations_2",
                          "Startup Cost": 0.0,
                          "Total Cost": 1.2,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.002,
                          "Actual Total Time": 0.002,
                          "Actual Rows": 1,
                          "Actual Loops": 10054,
                          "Filter": "((id = orderbook_orders.organization_id) AND ((verification_status)::text = 'APPROVED'::text))",
                          "Rows Removed by Filter": 3,
                          "Shared Hit Blocks": 10054,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0
                        },
                        {
                          "Node Type": "Index Scan",
                          "Parent Relationship": "Outer",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Scan Direction": "Forward",
                          "Index Name": "ix_users_organization_id_id",
                          "Relation Name": "users",
                          "Alias": "users_2",
                          "Startup Cost": 1.33,
                          "Total Cost": 10.5,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.003,
                          "Actual Total Time": 0.003,
                          "Actual Rows": 1,
                          "Actual Loops": 10053,
                          "Index Cond": "((organization_id = orderbook_orders.organization_id) AND (id = orderbook_orders.owner_user_id))",
                          "Rows Removed by Index Recheck": 0,
                          "Filter": "((email_verified IS TRUE) AND (must_change_password IS FALSE) AND ((role)::text = ANY ('{BUYER,SUPPLIER}'::text[])) AND ((kyc_status)::text <> 'REJECTED'::text) AND ((status)::text = 'APPROVED'::text) AND ((((kyc_status)::text = 'APPROVED'::text) AND (kyc_organization_id = organization_id)) OR (ANY ((id = (hashed SubPlan 7).col1) AND (organization_id = (hashed SubPlan 7).col2)))))",
                          "Rows Removed by Filter": 0,
                          "Shared Hit Blocks": 20106,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0,
                          "Plans": [
                            {
                              "Node Type": "Seq Scan",
                              "Parent Relationship": "SubPlan",
                              "Subplan Name": "SubPlan 7",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Relation Name": "kyc_execution_grace",
                              "Alias": "kyc_execution_grace_1",
                              "Startup Cost": 0.0,
                              "Total Cost": 1.1,
                              "Plan Rows": 6,
                              "Plan Width": 32,
                              "Actual Startup Time": 0.008,
                              "Actual Total Time": 0.01,
                              "Actual Rows": 6,
                              "Actual Loops": 1,
                              "Filter": "(expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone)",
                              "Rows Removed by Filter": 2,
                              "Shared Hit Blocks": 1,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            }
                          ]
                        }
                      ]
                    }
                  ]
                }
              ]
            },
            {
              "Node Type": "Materialize",
              "Parent Relationship": "Inner",
              "Parallel Aware": false,
              "Async Capable": false,
              "Startup Cost": 0.0,
              "Total Cost": 1.19,
              "Plan Rows": 13,
              "Plan Width": 263,
              "Actual Startup Time": 0.0,
              "Actual Total Time": 0.0,
              "Actual Rows": 4,
              "Actual Loops": 10045,
              "Shared Hit Blocks": 1,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0,
              "Plans": [
                {
                  "Node Type": "Seq Scan",
                  "Parent Relationship": "Outer",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Relation Name": "organizations",
                  "Alias": "organizations_1",
                  "Startup Cost": 0.0,
                  "Total Cost": 1.13,
                  "Plan Rows": 13,
                  "Plan Width": 263,
                  "Actual Startup Time": 0.007,
                  "Actual Total Time": 0.01,
                  "Actual Rows": 13,
                  "Actual Loops": 1,
                  "Shared Hit Blocks": 1,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0
                }
              ]
            }
          ]
        }
      ]
    }
  ]
}
```

### python /api/orderbook rows=10000

```sql
SELECT orderbook_orders.product_id, products.name AS product_name, products.fuel_type AS product_fuel_type, products.fuel_grade AS product_fuel_grade, products.is_active AS product_is_active, orderbook_orders.side, CASE WHEN (products.id = $1::UUID AND products.name = $2::VARCHAR AND products.fuel_type = $3::VARCHAR AND products.fuel_grade = $4::VARCHAR) THEN $5::VARCHAR WHEN (products.id = $6::UUID AND products.name = $7::VARCHAR AND products.fuel_type = $8::VARCHAR AND products.fuel_grade = $9::VARCHAR) THEN $10::VARCHAR WHEN (products.id = $11::UUID AND products.name = $12::VARCHAR AND products.fuel_type = $13::VARCHAR AND products.fuel_grade = $14::VARCHAR) THEN $15::VARCHAR WHEN (products.id = $16::UUID AND products.name = $17::VARCHAR AND products.fuel_type = $18::VARCHAR AND products.fuel_grade = $19::VARCHAR) THEN $20::VARCHAR WHEN (products.id = $21::UUID AND products.name = $22::VARCHAR AND products.fuel_type = $23::VARCHAR AND products.fuel_grade = $24::VARCHAR) THEN $25::VARCHAR WHEN (products.id = $26::UUID AND products.name = $27::VARCHAR AND products.fuel_type = $28::VARCHAR AND products.fuel_grade = $29::VARCHAR) THEN $30::VARCHAR END AS market_product, orderbook_orders.delivery_point_id, orderbook_orders.availability_window, orderbook_orders.status, orderbook_orders.expires_at, orderbook_orders.remaining_quantity_mt, orderbook_orders.price_per_mt_usd, orderbook_orders.off_spec, orderbook_orders.certification_declared, orderbook_orders.certification_scheme, orderbook_orders.specification_standard, orderbook_orders.msds_available, orderbook_orders.carbon_intensity_gco2_mj, orderbook_orders.carbon_intensity_method, orderbook_orders.feedstock, orderbook_orders.origin, orderbook_orders.fame_terms, orderbook_orders.lng_terms 
FROM orderbook_orders JOIN products ON orderbook_orders.product_id = products.id JOIN delivery_points ON orderbook_orders.delivery_point_id = delivery_points.id 
WHERE (orderbook_orders.side, CASE WHEN (products.id = $1::UUID AND products.name = $2::VARCHAR AND products.fuel_type = $3::VARCHAR AND products.fuel_grade = $4::VARCHAR) THEN $5::VARCHAR WHEN (products.id = $6::UUID AND products.name = $7::VARCHAR AND products.fuel_type = $8::VARCHAR AND products.fuel_grade = $9::VARCHAR) THEN $10::VARCHAR WHEN (products.id = $11::UUID AND products.name = $12::VARCHAR AND products.fuel_type = $13::VARCHAR AND products.fuel_grade = $14::VARCHAR) THEN $15::VARCHAR WHEN (products.id = $16::UUID AND products.name = $17::VARCHAR AND products.fuel_type = $18::VARCHAR AND products.fuel_grade = $19::VARCHAR) THEN $20::VARCHAR WHEN (products.id = $21::UUID AND products.name = $22::VARCHAR AND products.fuel_type = $23::VARCHAR AND products.fuel_grade = $24::VARCHAR) THEN $25::VARCHAR WHEN (products.id = $26::UUID AND products.name = $27::VARCHAR AND products.fuel_type = $28::VARCHAR AND products.fuel_grade = $29::VARCHAR) THEN $30::VARCHAR END, orderbook_orders.delivery_point_id, orderbook_orders.availability_window) IN (($105, $106, $107, $108)) AND orderbook_orders.status IN ($109, $110) AND orderbook_orders.remaining_quantity_mt > $31::INTEGER AND (orderbook_orders.expires_at > $32::TIMESTAMP WITH TIME ZONE OR orderbook_orders.expires_at IS NULL AND orderbook_orders.provenance IN ($111, $112)) AND CASE WHEN (orderbook_orders.creation_method = $33) THEN EXISTS (SELECT users.id 
FROM users 
WHERE orderbook_orders.creation_method = $34 AND users.id = orderbook_orders.owner_user_id AND users.id = orderbook_orders.created_by_actor_user_id AND users.role = $35 AND users.status = $36 AND (EXISTS (SELECT users_1.id 
FROM users AS users_1 
WHERE users_1.organization_id = orderbook_orders.organization_id AND users_1.role IN ($113, $114) AND users_1.status = $37 AND users_1.email_verified IS true AND users_1.must_change_password IS false AND users_1.kyc_status != $38::VARCHAR AND (users_1.kyc_status = $39::VARCHAR AND users_1.kyc_organization_id = users_1.organization_id OR (EXISTS (SELECT kyc_execution_grace.id 
FROM kyc_execution_grace 
WHERE kyc_execution_grace.user_id = users_1.id AND kyc_execution_grace.organization_id = users_1.organization_id AND kyc_execution_grace.expires_at > $40::TIMESTAMP WITH TIME ZONE))))) AND (EXISTS (SELECT organizations.id 
FROM organizations 
WHERE organizations.id = orderbook_orders.organization_id AND organizations.verification_status = $41::VARCHAR AND organizations.provenance = $42))) ELSE orderbook_orders.owner_user_id IS NULL OR (EXISTS (SELECT users.id 
FROM users 
WHERE users.id = orderbook_orders.owner_user_id AND users.organization_id = orderbook_orders.organization_id AND users.role IN ($115, $116) AND users.status = $43 AND users.email_verified IS true AND users.must_change_password IS false AND users.kyc_status != $44::VARCHAR AND (users.kyc_status = $45::VARCHAR AND users.kyc_organization_id = users.organization_id OR (EXISTS (SELECT kyc_execution_grace.id 
FROM kyc_execution_grace 
WHERE kyc_execution_grace.user_id = users.id AND kyc_execution_grace.organization_id = users.organization_id AND kyc_execution_grace.expires_at > $46::TIMESTAMP WITH TIME ZONE))) AND (EXISTS (SELECT organizations.id 
FROM organizations 
WHERE organizations.id = orderbook_orders.organization_id AND organizations.verification_status = $47::VARCHAR)))) END AND (orderbook_orders.product_id != $48::UUID OR orderbook_orders.delivery_point_id IN ($117::UUID) AND CAST((orderbook_orders.fame_terms ->> $49::TEXT) AS VARCHAR) = CAST(orderbook_orders.side AS VARCHAR) AND orderbook_orders.off_spec IS false AND (orderbook_orders.side != $50 OR CAST((orderbook_orders.fame_terms ->> $51::TEXT) AS VARCHAR) >= $52::VARCHAR AND orderbook_orders.msds_available IS true AND orderbook_orders.certification_declared IS true AND length(trim(coalesce(orderbook_orders.certification_scheme, $53::VARCHAR))) > $54::INTEGER)) AND (orderbook_orders.product_id != $55::UUID OR orderbook_orders.delivery_point_id IN ($118::UUID) AND CAST((orderbook_orders.lng_terms ->> $56::TEXT) AS VARCHAR) = CAST(orderbook_orders.side AS VARCHAR) AND orderbook_orders.off_spec IS false) AND orderbook_orders.provenance IN ($119, $120) AND products.is_active IS true AND (products.id = $57::UUID AND products.name = $58::VARCHAR AND products.fuel_type = $59::VARCHAR AND products.fuel_grade = $60::VARCHAR OR products.id = $61::UUID AND products.name = $62::VARCHAR AND products.fuel_type = $63::VARCHAR AND products.fuel_grade = $64::VARCHAR OR products.id = $65::UUID AND products.name = $66::VARCHAR AND products.fuel_type = $67::VARCHAR AND products.fuel_grade = $68::VARCHAR OR products.id = $69::UUID AND products.name = $70::VARCHAR AND products.fuel_type = $71::VARCHAR AND products.fuel_grade = $72::VARCHAR OR products.id = $73::UUID AND products.name = $74::VARCHAR AND products.fuel_type = $75::VARCHAR AND products.fuel_grade = $76::VARCHAR OR products.id = $77::UUID AND products.name = $78::VARCHAR AND products.fuel_type = $79::VARCHAR AND products.fuel_grade = $80::VARCHAR) AND delivery_points.is_active IS true AND (delivery_points.id = $81::UUID AND delivery_points.name = $82::VARCHAR AND delivery_points.region = $83::VARCHAR OR delivery_points.id = $84::UUID AND delivery_points.name = $85::VARCHAR AND delivery_points.region = $86::VARCHAR OR delivery_points.id = $87::UUID AND delivery_points.name = $88::VARCHAR AND delivery_points.region = $89::VARCHAR OR delivery_points.id = $90::UUID AND delivery_points.name = $91::VARCHAR AND delivery_points.region = $92::VARCHAR OR delivery_points.id = $93::UUID AND delivery_points.name = $94::VARCHAR AND delivery_points.region = $95::VARCHAR OR delivery_points.id = $96::UUID AND delivery_points.name = $97::VARCHAR AND delivery_points.region = $98::VARCHAR OR delivery_points.id = $99::UUID AND delivery_points.name = $100::VARCHAR AND delivery_points.region = $101::VARCHAR OR delivery_points.id = $102::UUID AND delivery_points.name = $103::VARCHAR AND delivery_points.region = $104::VARCHAR)
```

```json
{
  "Node Type": "Nested Loop",
  "Parallel Aware": false,
  "Async Capable": false,
  "Join Type": "Inner",
  "Startup Cost": 0.29,
  "Total Cost": 52438.28,
  "Plan Rows": 710,
  "Plan Width": 1775,
  "Actual Startup Time": 0.272,
  "Actual Total Time": 128.604,
  "Actual Rows": 10000,
  "Actual Loops": 1,
  "Inner Unique": false,
  "Shared Hit Blocks": 30280,
  "Shared Read Blocks": 0,
  "Shared Dirtied Blocks": 0,
  "Shared Written Blocks": 0,
  "Local Hit Blocks": 0,
  "Local Read Blocks": 0,
  "Local Dirtied Blocks": 0,
  "Local Written Blocks": 0,
  "Temp Read Blocks": 0,
  "Temp Written Blocks": 0,
  "Plans": [
    {
      "Node Type": "Seq Scan",
      "Parent Relationship": "Outer",
      "Parallel Aware": false,
      "Async Capable": false,
      "Relation Name": "delivery_points",
      "Alias": "delivery_points",
      "Startup Cost": 0.0,
      "Total Cost": 1.65,
      "Plan Rows": 1,
      "Plan Width": 16,
      "Actual Startup Time": 0.02,
      "Actual Total Time": 0.024,
      "Actual Rows": 1,
      "Actual Loops": 1,
      "Filter": "((is_active IS TRUE) AND (id = '73835e92-820e-584b-8280-bb61c63aa28e'::uuid) AND (((id = '78281fb3-e726-5396-802e-34b01fda21e8'::uuid) AND ((name)::text = 'Dalian'::text) AND ((region)::text = 'Asia'::text)) OR ((id = '262d36ae-6f35-5785-b9e8-9e221b0f1b78'::uuid) AND ((name)::text = 'Busan'::text) AND ((region)::text = 'Asia'::text)) OR ((id = '633c0593-f9b9-52ad-abe5-a0285f6888d3'::uuid) AND ((name)::text = 'Shanghai'::text) AND ((region)::text = 'Asia'::text)) OR ((id = '73835e92-820e-584b-8280-bb61c63aa28e'::uuid) AND ((name)::text = 'Singapore'::text) AND ((region)::text = 'Asia'::text)) OR ((id = '1379d36c-1ca9-55b7-9c0d-5235a0ba1f36'::uuid) AND ((name)::text = 'Rotterdam'::text) AND ((region)::text = 'Europe'::text)) OR ((id = 'a083db06-b050-56c2-a274-3897eac2fdae'::uuid) AND ((name)::text = 'Houston'::text) AND ((region)::text = 'Americas'::text)) OR ((id = '1be2a5cb-fc34-5e88-8b15-7f7d0fd6e534'::uuid) AND ((name)::text = 'Los Angeles'::text) AND ((region)::text = 'Americas'::text)) OR ((id = 'a4762444-4e0b-5bc1-b647-a185047adae5'::uuid) AND ((name)::text = 'Santos'::text) AND ((region)::text = 'Americas'::text))))",
      "Rows Removed by Filter": 8,
      "Shared Hit Blocks": 1,
      "Shared Read Blocks": 0,
      "Shared Dirtied Blocks": 0,
      "Shared Written Blocks": 0,
      "Local Hit Blocks": 0,
      "Local Read Blocks": 0,
      "Local Dirtied Blocks": 0,
      "Local Written Blocks": 0,
      "Temp Read Blocks": 0,
      "Temp Written Blocks": 0
    },
    {
      "Node Type": "Nested Loop",
      "Parent Relationship": "Inner",
      "Parallel Aware": false,
      "Async Capable": false,
      "Join Type": "Inner",
      "Startup Cost": 0.29,
      "Total Cost": 52386.92,
      "Plan Rows": 710,
      "Plan Width": 1759,
      "Actual Startup Time": 0.245,
      "Actual Total Time": 122.788,
      "Actual Rows": 10000,
      "Actual Loops": 1,
      "Inner Unique": false,
      "Shared Hit Blocks": 30279,
      "Shared Read Blocks": 0,
      "Shared Dirtied Blocks": 0,
      "Shared Written Blocks": 0,
      "Local Hit Blocks": 0,
      "Local Read Blocks": 0,
      "Local Dirtied Blocks": 0,
      "Local Written Blocks": 0,
      "Temp Read Blocks": 0,
      "Temp Written Blocks": 0,
      "Plans": [
        {
          "Node Type": "Seq Scan",
          "Parent Relationship": "Outer",
          "Parallel Aware": false,
          "Async Capable": false,
          "Relation Name": "products",
          "Alias": "products",
          "Startup Cost": 0.0,
          "Total Cost": 1.93,
          "Plan Rows": 1,
          "Plan Width": 41,
          "Actual Startup Time": 0.012,
          "Actual Total Time": 0.027,
          "Actual Rows": 1,
          "Actual Loops": 1,
          "Filter": "((is_active IS TRUE) AND (((id = '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid) AND ((name)::text = 'Bio-LNG'::text) AND ((fuel_type)::text = 'LNG'::text) AND ((fuel_grade)::text = 'Bio'::text)) OR ((id = 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) AND ((name)::text = 'UCOME B100'::text) AND ((fuel_type)::text = 'FAME'::text) AND ((fuel_grade)::text = 'UCOME'::text)) OR ((id = '9510c713-6e39-5080-add3-0a7c29657b79'::uuid) AND ((name)::text = 'Bio Methanol'::text) AND ((fuel_type)::text = 'Methanol'::text) AND ((fuel_grade)::text = 'Bio'::text)) OR ((id = 'f9b20492-b445-59cd-b292-a386d913f488'::uuid) AND ((name)::text = 'e-Methanol'::text) AND ((fuel_type)::text = 'Methanol'::text) AND ((fuel_grade)::text = 'E'::text)) OR ((id = 'c4a688be-f7c2-5edc-8f93-6b34e387609c'::uuid) AND ((name)::text = 'Bio Ethanol'::text) AND ((fuel_type)::text = 'Ethanol'::text) AND ((fuel_grade)::text = 'Bio'::text)) OR ((id = 'd186bffb-766d-5944-8825-989abbdcfc46'::uuid) AND ((name)::text = 'Synthetic Ethanol'::text) AND ((fuel_type)::text = 'Ethanol'::text) AND ((fuel_grade)::text = 'Synthetic'::text))) AND ((CASE WHEN ((id = '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid) AND ((name)::text = 'Bio-LNG'::text) AND ((fuel_type)::text = 'LNG'::text) AND ((fuel_grade)::text = 'Bio'::text)) THEN 'BIO_LNG'::character varying WHEN ((id = 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) AND ((name)::text = 'UCOME B100'::text) AND ((fuel_type)::text = 'FAME'::text) AND ((fuel_grade)::text = 'UCOME'::text)) THEN 'UCOME_B100'::character varying WHEN ((id = '9510c713-6e39-5080-add3-0a7c29657b79'::uuid) AND ((name)::text = 'Bio Methanol'::text) AND ((fuel_type)::text = 'Methanol'::text) AND ((fuel_grade)::text = 'Bio'::text)) THEN 'BIO_METHANOL'::character varying WHEN ((id = 'f9b20492-b445-59cd-b292-a386d913f488'::uuid) AND ((name)::text = 'e-Methanol'::text) AND ((fuel_type)::text = 'Methanol'::text) AND ((fuel_grade)::text = 'E'::text)) THEN 'E_METHANOL'::character varying WHEN ((id = 'c4a688be-f7c2-5edc-8f93-6b34e387609c'::uuid) AND ((name)::text = 'Bio Ethanol'::text) AND ((fuel_type)::text = 'Ethanol'::text) AND ((fuel_grade)::text = 'Bio'::text)) THEN 'BIO_ETHANOL'::character varying WHEN ((id = 'd186bffb-766d-5944-8825-989abbdcfc46'::uuid) AND ((name)::text = 'Synthetic Ethanol'::text) AND ((fuel_type)::text = 'Ethanol'::text) AND ((fuel_grade)::text = 'Synthetic'::text)) THEN 'SYNTHETIC_ETHANOL'::character varying ELSE NULL::character varying END)::text = 'BIO_METHANOL'::text))",
          "Rows Removed by Filter": 6,
          "Shared Hit Blocks": 1,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0
        },
        {
          "Node Type": "Index Scan",
          "Parent Relationship": "Inner",
          "Parallel Aware": false,
          "Async Capable": false,
          "Scan Direction": "Forward",
          "Index Name": "ix_orderbook_orders_public_aggregate",
          "Relation Name": "orderbook_orders",
          "Alias": "orderbook_orders",
          "Startup Cost": 0.29,
          "Total Cost": 52375.06,
          "Plan Rows": 994,
          "Plan Width": 1718,
          "Actual Startup Time": 0.231,
          "Actual Total Time": 119.843,
          "Actual Rows": 10000,
          "Actual Loops": 1,
          "Index Cond": "(((status)::text = ANY ('{OPEN,PARTIALLY_FILLED}'::text[])) AND ((provenance)::text = ANY ('{REAL,DEMO}'::text[])) AND (product_id = products.id) AND (delivery_point_id = '73835e92-820e-584b-8280-bb61c63aa28e'::uuid) AND ((availability_window)::text = '2031-04'::text) AND ((side)::text = 'BID'::text))",
          "Rows Removed by Index Recheck": 0,
          "Filter": "((remaining_quantity_mt > '0'::numeric) AND ((expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone) OR ((expires_at IS NULL) AND ((provenance)::text = ANY ('{REAL,UNKNOWN}'::text[])))) AND ((product_id <> '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid) OR ((delivery_point_id = '1379d36c-1ca9-55b7-9c0d-5235a0ba1f36'::uuid) AND ((lng_terms ->> 'side'::text) = (side)::text) AND (off_spec IS FALSE))) AND ((product_id <> 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) OR ((delivery_point_id = '73835e92-820e-584b-8280-bb61c63aa28e'::uuid) AND ((fame_terms ->> 'side'::text) = (side)::text) AND (off_spec IS FALSE) AND (((side)::text <> 'ASK'::text) OR (((fame_terms ->> 'certificate_valid_until'::text) >= '2026-10-15'::text) AND (msds_available IS TRUE) AND (certification_declared IS TRUE) AND (length(TRIM(BOTH FROM COALESCE(certification_scheme, ''::character varying))) > 0))))) AND CASE WHEN ((creation_method)::text = 'MARKET_SUPPORT'::text) THEN EXISTS(SubPlan 5) ELSE ((owner_user_id IS NULL) OR EXISTS(SubPlan 9)) END)",
          "Rows Removed by Filter": 0,
          "Shared Hit Blocks": 30278,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0,
          "Plans": [
            {
              "Node Type": "Result",
              "Parent Relationship": "SubPlan",
              "Subplan Name": "SubPlan 5",
              "Parallel Aware": false,
              "Async Capable": false,
              "Startup Cost": 14.05,
              "Total Cost": 15.45,
              "Plan Rows": 1,
              "Plan Width": 0,
              "Actual Startup Time": 0.0,
              "Actual Total Time": 0.0,
              "Actual Rows": 0,
              "Actual Loops": 0,
              "One-Time Filter": "(((orderbook_orders.creation_method)::text = 'MARKET_SUPPORT'::text) AND (InitPlan 3).col1 AND (InitPlan 4).col1 AND (orderbook_orders.owner_user_id = orderbook_orders.created_by_actor_user_id))",
              "Shared Hit Blocks": 0,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0,
              "Plans": [
                {
                  "Node Type": "Index Scan",
                  "Parent Relationship": "InitPlan",
                  "Subplan Name": "InitPlan 3",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Scan Direction": "Forward",
                  "Index Name": "ix_users_organization_id_id",
                  "Relation Name": "users",
                  "Alias": "users_1",
                  "Startup Cost": 0.14,
                  "Total Cost": 12.82,
                  "Plan Rows": 1,
                  "Plan Width": 0,
                  "Actual Startup Time": 0.0,
                  "Actual Total Time": 0.0,
                  "Actual Rows": 0,
                  "Actual Loops": 0,
                  "Index Cond": "(organization_id = orderbook_orders.organization_id)",
                  "Rows Removed by Index Recheck": 0,
                  "Filter": "((email_verified IS TRUE) AND (must_change_password IS FALSE) AND ((role)::text = ANY ('{BUYER,SUPPLIER}'::text[])) AND ((kyc_status)::text <> 'REJECTED'::text) AND ((status)::text = 'APPROVED'::text) AND ((((kyc_status)::text = 'APPROVED'::text) AND (kyc_organization_id = organization_id)) OR (ANY ((id = (hashed SubPlan 2).col1) AND (organization_id = (hashed SubPlan 2).col2)))))",
                  "Rows Removed by Filter": 0,
                  "Shared Hit Blocks": 0,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0,
                  "Plans": [
                    {
                      "Node Type": "Seq Scan",
                      "Parent Relationship": "SubPlan",
                      "Subplan Name": "SubPlan 2",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Relation Name": "kyc_execution_grace",
                      "Alias": "kyc_execution_grace",
                      "Startup Cost": 0.0,
                      "Total Cost": 1.1,
                      "Plan Rows": 6,
                      "Plan Width": 32,
                      "Actual Startup Time": 0.0,
                      "Actual Total Time": 0.0,
                      "Actual Rows": 0,
                      "Actual Loops": 0,
                      "Filter": "(expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone)",
                      "Rows Removed by Filter": 0,
                      "Shared Hit Blocks": 0,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0
                    }
                  ]
                },
                {
                  "Node Type": "Seq Scan",
                  "Parent Relationship": "InitPlan",
                  "Subplan Name": "InitPlan 4",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Relation Name": "organizations",
                  "Alias": "organizations",
                  "Startup Cost": 0.0,
                  "Total Cost": 1.23,
                  "Plan Rows": 1,
                  "Plan Width": 0,
                  "Actual Startup Time": 0.0,
                  "Actual Total Time": 0.0,
                  "Actual Rows": 0,
                  "Actual Loops": 0,
                  "Filter": "((id = orderbook_orders.organization_id) AND ((verification_status)::text = 'APPROVED'::text) AND ((provenance)::text = 'REAL'::text))",
                  "Rows Removed by Filter": 0,
                  "Shared Hit Blocks": 0,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0
                },
                {
                  "Node Type": "Seq Scan",
                  "Parent Relationship": "Outer",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Relation Name": "users",
                  "Alias": "users",
                  "Startup Cost": 14.05,
                  "Total Cost": 15.45,
                  "Plan Rows": 1,
                  "Plan Width": 0,
                  "Actual Startup Time": 0.0,
                  "Actual Total Time": 0.0,
                  "Actual Rows": 0,
                  "Actual Loops": 0,
                  "Filter": "((id = orderbook_orders.created_by_actor_user_id) AND ((role)::text = 'ADMIN'::text) AND ((status)::text = 'APPROVED'::text))",
                  "Rows Removed by Filter": 0,
                  "Shared Hit Blocks": 0,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0
                }
              ]
            },
            {
              "Node Type": "Result",
              "Parent Relationship": "SubPlan",
              "Subplan Name": "SubPlan 9",
              "Parallel Aware": false,
              "Async Capable": false,
              "Startup Cost": 1.33,
              "Total Cost": 10.5,
              "Plan Rows": 1,
              "Plan Width": 0,
              "Actual Startup Time": 0.01,
              "Actual Total Time": 0.01,
              "Actual Rows": 1,
              "Actual Loops": 10000,
              "One-Time Filter": "(InitPlan 8).col1",
              "Shared Hit Blocks": 30000,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0,
              "Plans": [
                {
                  "Node Type": "Seq Scan",
                  "Parent Relationship": "InitPlan",
                  "Subplan Name": "InitPlan 8",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Relation Name": "organizations",
                  "Alias": "organizations_1",
                  "Startup Cost": 0.0,
                  "Total Cost": 1.2,
                  "Plan Rows": 1,
                  "Plan Width": 0,
                  "Actual Startup Time": 0.005,
                  "Actual Total Time": 0.005,
                  "Actual Rows": 1,
                  "Actual Loops": 10000,
                  "Filter": "((id = orderbook_orders.organization_id) AND ((verification_status)::text = 'APPROVED'::text))",
                  "Rows Removed by Filter": 3,
                  "Shared Hit Blocks": 10000,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0
                },
                {
                  "Node Type": "Index Scan",
                  "Parent Relationship": "Outer",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Scan Direction": "Forward",
                  "Index Name": "ix_users_organization_id_id",
                  "Relation Name": "users",
                  "Alias": "users_2",
                  "Startup Cost": 1.33,
                  "Total Cost": 10.5,
                  "Plan Rows": 1,
                  "Plan Width": 0,
                  "Actual Startup Time": 0.003,
                  "Actual Total Time": 0.003,
                  "Actual Rows": 1,
                  "Actual Loops": 10000,
                  "Index Cond": "((organization_id = orderbook_orders.organization_id) AND (id = orderbook_orders.owner_user_id))",
                  "Rows Removed by Index Recheck": 0,
                  "Filter": "((email_verified IS TRUE) AND (must_change_password IS FALSE) AND ((role)::text = ANY ('{BUYER,SUPPLIER}'::text[])) AND ((kyc_status)::text <> 'REJECTED'::text) AND ((status)::text = 'APPROVED'::text) AND ((((kyc_status)::text = 'APPROVED'::text) AND (kyc_organization_id = organization_id)) OR (ANY ((id = (hashed SubPlan 7).col1) AND (organization_id = (hashed SubPlan 7).col2)))))",
                  "Rows Removed by Filter": 0,
                  "Shared Hit Blocks": 20000,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0,
                  "Plans": [
                    {
                      "Node Type": "Seq Scan",
                      "Parent Relationship": "SubPlan",
                      "Subplan Name": "SubPlan 7",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Relation Name": "kyc_execution_grace",
                      "Alias": "kyc_execution_grace_1",
                      "Startup Cost": 0.0,
                      "Total Cost": 1.1,
                      "Plan Rows": 6,
                      "Plan Width": 32,
                      "Actual Startup Time": 0.0,
                      "Actual Total Time": 0.0,
                      "Actual Rows": 0,
                      "Actual Loops": 0,
                      "Filter": "(expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone)",
                      "Rows Removed by Filter": 0,
                      "Shared Hit Blocks": 0,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0
                    }
                  ]
                }
              ]
            }
          ]
        }
      ]
    }
  ]
}
```

### rust /api/orderbook rows=50

```sql
WITH eligible_orders AS (-- Shared public-order visibility projection (Task 6).
--
-- This is the complete bound internal relation: every caller value is a parameter, so
-- no route ever concatenates text into SQL. Parameters are
--   $1  attested market policy (jsonb text)
--   $2  captured UTC instant (timestamptz)
--   $3  Singapore date (text, YYYY-MM-DD)
--   $4  include_off_spec (boolean)
--   $5  product_id (uuid, null = any)
--   $6  delivery_point_id (uuid, null = any)
--   $7  side (text, null = any)
--   $8  fuel_type (text, null = any)
--   $9  market_product code (text, null = any)
--   $10 region (text, null = any)
--   $11 availability_window (text, null = any)
--   $12 surface: orderbook, listings, or candidates (text)
--   $13 runtime KYC execution gate enabled (boolean)
-- No pagination here: consumers rank, aggregate and page the shared relation.
--
-- Product identity comes only from the policy. The statement never names a
-- product, a product code, or a product UUID.
WITH policy_products AS (
    SELECT *
    FROM jsonb_to_recordset(($1::jsonb) -> 'products') AS policy_product(
        id uuid,
        code text,
        name text,
        fuel_type text,
        fuel_grade text,
        execution_mode text,
        lanes jsonb,
        qualification_profile text,
        display_order integer
    )
),
policy_points AS (
    SELECT *
    FROM jsonb_to_recordset(($1::jsonb) -> 'points') AS policy_point(
        id uuid,
        name text,
        region text,
        timezone text,
        display_order integer
    )
)
SELECT o.*, p.name AS product_name, p.fuel_type AS product_fuel_type,
       p.fuel_grade AS product_fuel_grade, p.is_active AS product_is_active,
       dp.name AS delivery_point_name, dp.region AS delivery_point_region,
       (SELECT pp.code FROM policy_products pp WHERE pp.id = p.id) AS market_product,
       -- Internal only. Never serialize this object as a public DTO.
       to_jsonb(o) || jsonb_build_object(
           'quantity_mt', o.quantity_mt::text,
           'remaining_quantity_mt', o.remaining_quantity_mt::text,
           'price_per_mt_usd', o.price_per_mt_usd::text,
           'carbon_intensity_gco2_mj', o.carbon_intensity_gco2_mj::text,
           'energy_density_mj_kg', o.energy_density_mj_kg::text
       ) AS private_digest_input
FROM orderbook_orders AS o
JOIN products AS p ON p.id = o.product_id
JOIN delivery_points AS dp ON dp.id = o.delivery_point_id
WHERE o.status IN ('OPEN', 'PARTIALLY_FILLED')
    -- Strict canonical product identity, active row, and approved lane.
    AND p.is_active
    AND EXISTS (
        SELECT 1
        FROM policy_products AS pp
        WHERE pp.execution_mode = 'ORDERBOOK'
          AND pp.id = p.id
          AND pp.name = p.name
          AND pp.fuel_type = p.fuel_type
          AND pp.fuel_grade = p.fuel_grade
          AND EXISTS (
              SELECT 1
              FROM jsonb_array_elements_text(pp.lanes) AS lane(value)
              WHERE lane.value = o.delivery_point_id::text
          )
    )
    -- Strict canonical delivery-point identity and active row.
    AND dp.is_active
    AND EXISTS (
        SELECT 1
        FROM policy_points AS pq
        WHERE pq.id = dp.id AND pq.name = dp.name AND pq.region = dp.region
    )
    -- Canonical availability-window grammar.
    AND ($12::text <> 'orderbook' OR (
        o.availability_window = 'SPOT'
        OR o.availability_window ~ '^[0-9]{4}-(0[1-9]|1[0-2])$'
        OR o.availability_window ~ '^[0-9]{4}-Q[1-4]$'
        OR o.availability_window ~ '^[0-9]{4}-CAL$'
    ))
    AND (-- Shared expiry, owner admission and shallow declaration rule.
-- Requires policy_products and alias o. Binds $2, $3 and $13.
TRUE
    -- Currency: an explicit future expiry, or a null expiry on REAL/UNKNOWN.
    AND (
        o.expires_at > $2::timestamptz
        OR (
            o.expires_at IS NULL
            AND o.provenance IN ('REAL', 'UNKNOWN')
        )
    )
    -- Owner admission. Assisted rows use the narrower administrator rule.
    AND (
        CASE
            WHEN o.creation_method = 'MARKET_SUPPORT' THEN EXISTS (
                SELECT 1
                FROM users AS actor
                WHERE actor.id = o.owner_user_id
                  AND actor.id = o.created_by_actor_user_id
                  AND actor.role = 'ADMIN'
                  AND actor.status = 'APPROVED'
                  AND EXISTS (
                      SELECT 1
                      FROM users AS member
                      WHERE member.organization_id = o.organization_id
                        AND member.role IN ('BUYER', 'SUPPLIER')
                        AND member.status = 'APPROVED'
                        AND member.email_verified
                        AND NOT member.must_change_password
                        AND member.kyc_status <> 'REJECTED'
                        AND CASE WHEN $13::boolean THEN (
                            (member.kyc_status = 'APPROVED'
                             AND member.kyc_organization_id = member.organization_id)
                            OR EXISTS (
                                SELECT 1 FROM kyc_execution_grace AS grace
                                WHERE grace.user_id = member.id
                                  AND grace.organization_id = member.organization_id
                                  AND grace.expires_at > $2::timestamptz
                            )
                        ) ELSE (
                            member.kyc_organization_id IS NULL
                            OR member.kyc_organization_id = member.organization_id
                        ) END
                  )
                  AND EXISTS (
                      SELECT 1
                      FROM organizations AS assisted_org
                      WHERE assisted_org.id = o.organization_id
                        AND assisted_org.verification_status = 'APPROVED'
                        AND assisted_org.provenance = 'REAL'
                  )
            )
            ELSE (
                o.owner_user_id IS NULL
                OR EXISTS (
                    SELECT 1
                    FROM users AS owner
                    WHERE owner.id = o.owner_user_id
                      AND owner.organization_id = o.organization_id
                      AND owner.role IN ('BUYER', 'SUPPLIER')
                      AND owner.status = 'APPROVED'
                      AND owner.email_verified
                      AND NOT owner.must_change_password
                      AND owner.kyc_status <> 'REJECTED'
                      AND CASE WHEN $13::boolean THEN (
                          (owner.kyc_status = 'APPROVED'
                           AND owner.kyc_organization_id = owner.organization_id)
                          OR EXISTS (
                              SELECT 1 FROM kyc_execution_grace AS grace
                              WHERE grace.user_id = owner.id
                                AND grace.organization_id = owner.organization_id
                                AND grace.expires_at > $2::timestamptz
                          )
                      ) ELSE (
                          owner.kyc_organization_id IS NULL
                          OR owner.kyc_organization_id = owner.organization_id
                      ) END
                      AND EXISTS (
                          SELECT 1
                          FROM organizations AS org
                          WHERE org.id = o.organization_id
                            AND org.verification_status = 'APPROVED'
                      )
                )
            )
        END
    )
    -- fame_v1 declaration currency, keyed by policy profile.
    AND (
        NOT EXISTS (
            SELECT 1
            FROM policy_products AS pf
            WHERE pf.qualification_profile = 'fame_v1'
              AND pf.id = o.product_id
        )
        OR (
            EXISTS (
                SELECT 1
                FROM policy_products AS pf
                WHERE pf.qualification_profile = 'fame_v1'
                  AND pf.id = o.product_id
                  AND EXISTS (
                      SELECT 1
                      FROM jsonb_array_elements_text(pf.lanes) AS lane(value)
                      WHERE lane.value = o.delivery_point_id::text
                  )
            )
            AND o.fame_terms ->> 'side' = o.side
            AND o.off_spec = false
            AND (
                o.side <> 'ASK'
                OR (
                    o.fame_terms ->> 'certificate_valid_until' >= $3::text
                    AND o.msds_available
                    AND o.certification_declared
                    AND length(btrim(coalesce(o.certification_scheme, ''))) > 0
                )
            )
        )
    )
    -- lng_v1 declaration currency, keyed by policy profile.
    AND (
        NOT EXISTS (
            SELECT 1
            FROM policy_products AS pl
            WHERE pl.qualification_profile = 'lng_v1'
              AND pl.id = o.product_id
        )
        OR (
            EXISTS (
                SELECT 1
                FROM policy_products AS pl
                WHERE pl.qualification_profile = 'lng_v1'
                  AND pl.id = o.product_id
                  AND EXISTS (
                      SELECT 1
                      FROM jsonb_array_elements_text(pl.lanes) AS lane(value)
                      WHERE lane.value = o.delivery_point_id::text
                  )
            )
            AND o.lng_terms ->> 'side' = o.side
            AND o.off_spec = false
        )
    )
)
    -- Independently labelled public rows.
    AND o.provenance IN ('REAL', 'DEMO')
    -- Admitted fuel types.
    AND p.fuel_type IN ('Methanol', 'Ethanol', 'FAME', 'LNG')
    -- Scalar ASK quality declarations.
    AND ($12::text <> 'orderbook' OR (
        o.side <> 'ASK'
        OR (
            length(btrim(coalesce(o.certification_scheme, ''))) > 0
            AND o.certification_declared
            AND length(btrim(coalesce(o.specification_standard, ''))) > 0
            AND o.msds_available
            AND (
                EXISTS (
                    SELECT 1
                    FROM policy_products AS pd
                    WHERE pd.qualification_profile IN ('fame_v1', 'lng_v1')
                      AND pd.id = p.id
                      AND pd.name = p.name
                      AND pd.fuel_type = p.fuel_type
                      AND pd.fuel_grade = p.fuel_grade
                )
                OR o.carbon_intensity_gco2_mj IS NOT NULL
            )
            AND length(btrim(coalesce(o.feedstock, ''))) > 0
            AND (
                EXISTS (
                    SELECT 1
                    FROM policy_products AS po
                    WHERE po.qualification_profile = 'fame_v1'
                      AND po.id = p.id
                      AND po.name = p.name
                      AND po.fuel_type = p.fuel_type
                      AND po.fuel_grade = p.fuel_grade
                )
                OR length(btrim(coalesce(o.origin, ''))) > 0
            )
        )
    )
    )
    -- Off-spec rows are hidden unless the caller asks for them.
    AND ($12::text <> 'orderbook' OR o.off_spec = false OR $4::boolean)
    AND ($12::text <> 'listings' OR o.side = 'ASK')
    AND $12::text IN ('orderbook', 'listings', 'candidates')
    -- Route filters.
    AND ($5::uuid IS NULL OR o.product_id = $5::uuid)
    AND ($6::uuid IS NULL OR o.delivery_point_id = $6::uuid)
    AND ($7::text IS NULL OR o.side = $7::text)
    AND ($8::text IS NULL OR p.fuel_type = $8::text)
    AND (
        $9::text IS NULL
        OR EXISTS (
            SELECT 1
            FROM policy_products AS pm
            WHERE pm.code = $9::text
              AND pm.id = p.id
              AND pm.name = p.name
              AND pm.fuel_type = p.fuel_type
              AND pm.fuel_grade = p.fuel_grade
        )
    )
    AND ($10::text IS NULL OR $10::text = '' OR dp.region = $10::text OR dp.name = $10::text)
    AND ($11::text IS NULL OR o.availability_window = $11::text)
) SELECT page.*, COALESCE(org.supplier_tier, 'INDEPENDENT') AS tier_label
FROM (
SELECT o.*
FROM eligible_orders o
ORDER BY
    CASE WHEN $14 = 'price_asc' THEN o.price_per_mt_usd END ASC,
    CASE WHEN $14 = 'price_desc' THEN o.price_per_mt_usd END DESC,
    CASE WHEN $14 IN ('price_asc', 'price_desc') THEN o.acceptance_ordinal END ASC,
    CASE WHEN $14 IN ('price_asc', 'price_desc') THEN o.id END ASC,
    CASE WHEN $14 = 'quantity_desc' THEN o.remaining_quantity_mt END DESC,
    CASE WHEN $14 IN ('newest', 'quantity_desc') THEN o.created_at END DESC,
    CASE WHEN $14 IN ('newest', 'quantity_desc') THEN o.id END DESC
LIMIT $15 OFFSET $16
) AS page
LEFT JOIN organizations org ON org.id = page.organization_id

ORDER BY
    CASE WHEN $14 = 'price_asc' THEN page.price_per_mt_usd END ASC,
    CASE WHEN $14 = 'price_desc' THEN page.price_per_mt_usd END DESC,
    CASE WHEN $14 IN ('price_asc', 'price_desc') THEN page.acceptance_ordinal END ASC,
    CASE WHEN $14 IN ('price_asc', 'price_desc') THEN page.id END ASC,
    CASE WHEN $14 = 'quantity_desc' THEN page.remaining_quantity_mt END DESC,
    CASE WHEN $14 IN ('newest', 'quantity_desc') THEN page.created_at END DESC,
    CASE WHEN $14 IN ('newest', 'quantity_desc') THEN page.id END DESC

```

```json
{
  "Node Type": "Nested Loop",
  "Parallel Aware": false,
  "Async Capable": false,
  "Join Type": "Left",
  "Startup Cost": 3937238.94,
  "Total Cost": 3937240.25,
  "Plan Rows": 1,
  "Plan Width": 3163,
  "Actual Startup Time": 1577.781,
  "Actual Total Time": 1578.248,
  "Actual Rows": 50,
  "Actual Loops": 1,
  "Inner Unique": true,
  "Join Filter": "(org.id = o.organization_id)",
  "Rows Removed by Join Filter": 150,
  "Shared Hit Blocks": 30526,
  "Shared Read Blocks": 0,
  "Shared Dirtied Blocks": 0,
  "Shared Written Blocks": 0,
  "Local Hit Blocks": 0,
  "Local Read Blocks": 0,
  "Local Dirtied Blocks": 0,
  "Local Written Blocks": 0,
  "Temp Read Blocks": 0,
  "Temp Written Blocks": 0,
  "Plans": [
    {
      "Node Type": "Limit",
      "Parent Relationship": "Outer",
      "Parallel Aware": false,
      "Async Capable": false,
      "Startup Cost": 3937238.94,
      "Total Cost": 3937238.95,
      "Plan Rows": 1,
      "Plan Width": 3131,
      "Actual Startup Time": 1577.766,
      "Actual Total Time": 1577.821,
      "Actual Rows": 50,
      "Actual Loops": 1,
      "Shared Hit Blocks": 30476,
      "Shared Read Blocks": 0,
      "Shared Dirtied Blocks": 0,
      "Shared Written Blocks": 0,
      "Local Hit Blocks": 0,
      "Local Read Blocks": 0,
      "Local Dirtied Blocks": 0,
      "Local Written Blocks": 0,
      "Temp Read Blocks": 0,
      "Temp Written Blocks": 0,
      "Plans": [
        {
          "Node Type": "Sort",
          "Parent Relationship": "Outer",
          "Parallel Aware": false,
          "Async Capable": false,
          "Startup Cost": 3937238.94,
          "Total Cost": 3937238.95,
          "Plan Rows": 1,
          "Plan Width": 3131,
          "Actual Startup Time": 1577.765,
          "Actual Total Time": 1577.807,
          "Actual Rows": 50,
          "Actual Loops": 1,
          "Sort Key": [
            "o.created_at DESC",
            "o.id DESC"
          ],
          "Sort Method": "top-N heapsort",
          "Sort Space Used": 215,
          "Sort Space Type": "Memory",
          "Shared Hit Blocks": 30476,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0,
          "Plans": [
            {
              "Node Type": "Subquery Scan",
              "Parent Relationship": "Outer",
              "Parallel Aware": false,
              "Async Capable": false,
              "Alias": "o",
              "Startup Cost": 1.0,
              "Total Cost": 3937238.93,
              "Plan Rows": 1,
              "Plan Width": 3131,
              "Actual Startup Time": 0.707,
              "Actual Total Time": 1545.122,
              "Actual Rows": 10045,
              "Actual Loops": 1,
              "Shared Hit Blocks": 30470,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0,
              "Plans": [
                {
                  "Node Type": "Nested Loop",
                  "Parent Relationship": "Subquery",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Join Type": "Semi",
                  "Startup Cost": 1.0,
                  "Total Cost": 3937238.93,
                  "Plan Rows": 1,
                  "Plan Width": 2987,
                  "Actual Startup Time": 0.703,
                  "Actual Total Time": 1537.259,
                  "Actual Rows": 10045,
                  "Actual Loops": 1,
                  "Inner Unique": false,
                  "Join Filter": "((policy_point.id = dp.id) AND ((dp.name)::text = policy_point.name) AND (policy_point.region = (dp.region)::text))",
                  "Rows Removed by Join Filter": 30139,
                  "Shared Hit Blocks": 30470,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0,
                  "Plans": [
                    {
                      "Node Type": "Function Scan",
                      "Parent Relationship": "InitPlan",
                      "Subplan Name": "CTE policy_products",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Function Name": "jsonb_to_recordset",
                      "Alias": "policy_product",
                      "Startup Cost": 0.0,
                      "Total Cost": 1.0,
                      "Plan Rows": 100,
                      "Plan Width": 244,
                      "Actual Startup Time": 0.099,
                      "Actual Total Time": 0.1,
                      "Actual Rows": 6,
                      "Actual Loops": 1,
                      "Shared Hit Blocks": 0,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0
                    },
                    {
                      "Node Type": "Nested Loop",
                      "Parent Relationship": "Outer",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Join Type": "Semi",
                      "Startup Cost": 0.0,
                      "Total Cost": 3937232.9,
                      "Plan Rows": 1,
                      "Plan Width": 5862,
                      "Actual Startup Time": 0.388,
                      "Actual Total Time": 377.882,
                      "Actual Rows": 10045,
                      "Actual Loops": 1,
                      "Inner Unique": false,
                      "Join Filter": "((pp.id = p.id) AND ((p.name)::text = pp.name) AND (pp.fuel_type = (p.fuel_type)::text) AND (pp.fuel_grade = (p.fuel_grade)::text) AND EXISTS(SubPlan 24))",
                      "Rows Removed by Join Filter": 20091,
                      "Shared Hit Blocks": 30470,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0,
                      "Plans": [
                        {
                          "Node Type": "Nested Loop",
                          "Parent Relationship": "Outer",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Join Type": "Inner",
                          "Startup Cost": 0.0,
                          "Total Cost": 3935526.59,
                          "Plan Rows": 956,
                          "Plan Width": 5862,
                          "Actual Startup Time": 0.335,
                          "Actual Total Time": 275.249,
                          "Actual Rows": 10047,
                          "Actual Loops": 1,
                          "Inner Unique": true,
                          "Join Filter": "((o_1.product_id = p.id) AND (((o_1.side)::text <> 'ASK'::text) OR ((length(btrim((COALESCE(o_1.certification_scheme, ''::character varying))::text)) > 0) AND o_1.certification_declared AND (length(btrim((COALESCE(o_1.specification_standard, ''::character varying))::text)) > 0) AND o_1.msds_available AND ((ANY ((p.id = (hashed SubPlan 21).col1) AND ((p.name)::text = (hashed SubPlan 21).col2) AND ((p.fuel_type)::text = (hashed SubPlan 21).col3) AND ((p.fuel_grade)::text = (hashed SubPlan 21).col4))) OR (o_1.carbon_intensity_gco2_mj IS NOT NULL)) AND (length(btrim((COALESCE(o_1.feedstock, ''::character varying))::text)) > 0) AND ((ANY ((p.id = (hashed SubPlan 23).col1) AND ((p.name)::text = (hashed SubPlan 23).col2) AND ((p.fuel_type)::text = (hashed SubPlan 23).col3) AND ((p.fuel_grade)::text = (hashed SubPlan 23).col4))) OR (length(btrim((COALESCE(o_1.origin, ''::character varying))::text)) > 0)))))",
                          "Rows Removed by Join Filter": 10081,
                          "Shared Hit Blocks": 30470,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0,
                          "Plans": [
                            {
                              "Node Type": "Nested Loop",
                              "Parent Relationship": "Outer",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Join Type": "Inner",
                              "Startup Cost": 0.0,
                              "Total Cost": 3896066.49,
                              "Plan Rows": 1118,
                              "Plan Width": 5821,
                              "Actual Startup Time": 0.309,
                              "Actual Total Time": 260.374,
                              "Actual Rows": 10049,
                              "Actual Loops": 1,
                              "Inner Unique": true,
                              "Join Filter": "(o_1.delivery_point_id = dp.id)",
                              "Rows Removed by Join Filter": 31,
                              "Shared Hit Blocks": 30469,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0,
                              "Plans": [
                                {
                                  "Node Type": "Seq Scan",
                                  "Parent Relationship": "Outer",
                                  "Parallel Aware": false,
                                  "Async Capable": false,
                                  "Relation Name": "orderbook_orders",
                                  "Alias": "o_1",
                                  "Startup Cost": 0.0,
                                  "Total Cost": 3895927.49,
                                  "Plan Rows": 1258,
                                  "Plan Width": 5790,
                                  "Actual Startup Time": 0.268,
                                  "Actual Total Time": 247.217,
                                  "Actual Rows": 10050,
                                  "Actual Loops": 1,
                                  "Filter": "((NOT off_spec) AND ((status)::text = ANY ('{OPEN,PARTIALLY_FILLED}'::text[])) AND ((provenance)::text = ANY ('{REAL,DEMO}'::text[])) AND ((expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone) OR ((expires_at IS NULL) AND ((provenance)::text = ANY ('{REAL,UNKNOWN}'::text[])))) AND (((availability_window)::text = 'SPOT'::text) OR ((availability_window)::text ~ '^[0-9]{4}-(0[1-9]|1[0-2])$'::text) OR ((availability_window)::text ~ '^[0-9]{4}-Q[1-4]$'::text) OR ((availability_window)::text ~ '^[0-9]{4}-CAL$'::text)) AND CASE WHEN ((creation_method)::text = 'MARKET_SUPPORT'::text) THEN EXISTS(SubPlan 7) ELSE ((owner_user_id IS NULL) OR EXISTS(SubPlan 11)) END AND ((NOT (ANY (product_id = (hashed SubPlan 17).col1))) OR (EXISTS(SubPlan 19) AND ((lng_terms ->> 'side'::text) = (side)::text) AND (NOT off_spec))) AND ((NOT (ANY (product_id = (hashed SubPlan 13).col1))) OR (EXISTS(SubPlan 15) AND ((fame_terms ->> 'side'::text) = (side)::text) AND (NOT off_spec) AND (((side)::text <> 'ASK'::text) OR (((fame_terms ->> 'certificate_valid_until'::text) >= '2026-10-15'::text) AND msds_available AND certification_declared AND (length(btrim((COALESCE(certification_scheme, ''::character varying))::text)) > 0))))))",
                                  "Rows Removed by Filter": 29,
                                  "Shared Hit Blocks": 30468,
                                  "Shared Read Blocks": 0,
                                  "Shared Dirtied Blocks": 0,
                                  "Shared Written Blocks": 0,
                                  "Local Hit Blocks": 0,
                                  "Local Read Blocks": 0,
                                  "Local Dirtied Blocks": 0,
                                  "Local Written Blocks": 0,
                                  "Temp Read Blocks": 0,
                                  "Temp Written Blocks": 0,
                                  "Plans": [
                                    {
                                      "Node Type": "Result",
                                      "Parent Relationship": "SubPlan",
                                      "Subplan Name": "SubPlan 7",
                                      "Parallel Aware": false,
                                      "Async Capable": false,
                                      "Startup Cost": 14.05,
                                      "Total Cost": 15.45,
                                      "Plan Rows": 1,
                                      "Plan Width": 0,
                                      "Actual Startup Time": 0.021,
                                      "Actual Total Time": 0.022,
                                      "Actual Rows": 0,
                                      "Actual Loops": 7,
                                      "One-Time Filter": "((InitPlan 5).col1 AND (InitPlan 6).col1 AND (o_1.owner_user_id = o_1.created_by_actor_user_id))",
                                      "Shared Hit Blocks": 22,
                                      "Shared Read Blocks": 0,
                                      "Shared Dirtied Blocks": 0,
                                      "Shared Written Blocks": 0,
                                      "Local Hit Blocks": 0,
                                      "Local Read Blocks": 0,
                                      "Local Dirtied Blocks": 0,
                                      "Local Written Blocks": 0,
                                      "Temp Read Blocks": 0,
                                      "Temp Written Blocks": 0,
                                      "Plans": [
                                        {
                                          "Node Type": "Index Scan",
                                          "Parent Relationship": "InitPlan",
                                          "Subplan Name": "InitPlan 5",
                                          "Parallel Aware": false,
                                          "Async Capable": false,
                                          "Scan Direction": "Forward",
                                          "Index Name": "ix_users_organization_id_id",
                                          "Relation Name": "users",
                                          "Alias": "member",
                                          "Startup Cost": 0.14,
                                          "Total Cost": 12.82,
                                          "Plan Rows": 1,
                                          "Plan Width": 0,
                                          "Actual Startup Time": 0.012,
                                          "Actual Total Time": 0.012,
                                          "Actual Rows": 1,
                                          "Actual Loops": 7,
                                          "Index Cond": "(organization_id = o_1.organization_id)",
                                          "Rows Removed by Index Recheck": 0,
                                          "Filter": "(email_verified AND (NOT must_change_password) AND ((role)::text = ANY ('{BUYER,SUPPLIER}'::text[])) AND ((kyc_status)::text <> 'REJECTED'::text) AND ((status)::text = 'APPROVED'::text) AND ((((kyc_status)::text = 'APPROVED'::text) AND (kyc_organization_id = organization_id)) OR (ANY ((id = (hashed SubPlan 4).col1) AND (organization_id = (hashed SubPlan 4).col2)))))",
                                          "Rows Removed by Filter": 0,
                                          "Shared Hit Blocks": 15,
                                          "Shared Read Blocks": 0,
                                          "Shared Dirtied Blocks": 0,
                                          "Shared Written Blocks": 0,
                                          "Local Hit Blocks": 0,
                                          "Local Read Blocks": 0,
                                          "Local Dirtied Blocks": 0,
                                          "Local Written Blocks": 0,
                                          "Temp Read Blocks": 0,
                                          "Temp Written Blocks": 0,
                                          "Plans": [
                                            {
                                              "Node Type": "Seq Scan",
                                              "Parent Relationship": "SubPlan",
                                              "Subplan Name": "SubPlan 4",
                                              "Parallel Aware": false,
                                              "Async Capable": false,
                                              "Relation Name": "kyc_execution_grace",
                                              "Alias": "grace",
                                              "Startup Cost": 0.0,
                                              "Total Cost": 1.1,
                                              "Plan Rows": 6,
                                              "Plan Width": 32,
                                              "Actual Startup Time": 0.006,
                                              "Actual Total Time": 0.008,
                                              "Actual Rows": 6,
                                              "Actual Loops": 1,
                                              "Filter": "(expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone)",
                                              "Rows Removed by Filter": 2,
                                              "Shared Hit Blocks": 1,
                                              "Shared Read Blocks": 0,
                                              "Shared Dirtied Blocks": 0,
                                              "Shared Written Blocks": 0,
                                              "Local Hit Blocks": 0,
                                              "Local Read Blocks": 0,
                                              "Local Dirtied Blocks": 0,
                                              "Local Written Blocks": 0,
                                              "Temp Read Blocks": 0,
                                              "Temp Written Blocks": 0
                                            }
                                          ]
                                        },
                                        {
                                          "Node Type": "Seq Scan",
                                          "Parent Relationship": "InitPlan",
                                          "Subplan Name": "InitPlan 6",
                                          "Parallel Aware": false,
                                          "Async Capable": false,
                                          "Relation Name": "organizations",
                                          "Alias": "assisted_org",
                                          "Startup Cost": 0.0,
                                          "Total Cost": 1.23,
                                          "Plan Rows": 1,
                                          "Plan Width": 0,
                                          "Actual Startup Time": 0.003,
                                          "Actual Total Time": 0.003,
                                          "Actual Rows": 1,
                                          "Actual Loops": 4,
                                          "Filter": "((id = o_1.organization_id) AND ((verification_status)::text = 'APPROVED'::text) AND ((provenance)::text = 'REAL'::text))",
                                          "Rows Removed by Filter": 4,
                                          "Shared Hit Blocks": 4,
                                          "Shared Read Blocks": 0,
                                          "Shared Dirtied Blocks": 0,
                                          "Shared Written Blocks": 0,
                                          "Local Hit Blocks": 0,
                                          "Local Read Blocks": 0,
                                          "Local Dirtied Blocks": 0,
                                          "Local Written Blocks": 0,
                                          "Temp Read Blocks": 0,
                                          "Temp Written Blocks": 0
                                        },
                                        {
                                          "Node Type": "Seq Scan",
                                          "Parent Relationship": "Outer",
                                          "Parallel Aware": false,
                                          "Async Capable": false,
                                          "Relation Name": "users",
                                          "Alias": "actor",
                                          "Startup Cost": 14.05,
                                          "Total Cost": 15.45,
                                          "Plan Rows": 1,
                                          "Plan Width": 0,
                                          "Actual Startup Time": 0.014,
                                          "Actual Total Time": 0.014,
                                          "Actual Rows": 1,
                                          "Actual Loops": 3,
                                          "Filter": "((id = o_1.created_by_actor_user_id) AND ((role)::text = 'ADMIN'::text) AND ((status)::text = 'APPROVED'::text))",
                                          "Rows Removed by Filter": 13,
                                          "Shared Hit Blocks": 3,
                                          "Shared Read Blocks": 0,
                                          "Shared Dirtied Blocks": 0,
                                          "Shared Written Blocks": 0,
                                          "Local Hit Blocks": 0,
                                          "Local Read Blocks": 0,
                                          "Local Dirtied Blocks": 0,
                                          "Local Written Blocks": 0,
                                          "Temp Read Blocks": 0,
                                          "Temp Written Blocks": 0
                                        }
                                      ]
                                    },
                                    {
                                      "Node Type": "Result",
                                      "Parent Relationship": "SubPlan",
                                      "Subplan Name": "SubPlan 11",
                                      "Parallel Aware": false,
                                      "Async Capable": false,
                                      "Startup Cost": 1.33,
                                      "Total Cost": 10.5,
                                      "Plan Rows": 1,
                                      "Plan Width": 0,
                                      "Actual Startup Time": 0.013,
                                      "Actual Total Time": 0.013,
                                      "Actual Rows": 1,
                                      "Actual Loops": 10060,
                                      "One-Time Filter": "(InitPlan 10).col1",
                                      "Shared Hit Blocks": 30178,
                                      "Shared Read Blocks": 0,
                                      "Shared Dirtied Blocks": 0,
                                      "Shared Written Blocks": 0,
                                      "Local Hit Blocks": 0,
                                      "Local Read Blocks": 0,
                                      "Local Dirtied Blocks": 0,
                                      "Local Written Blocks": 0,
                                      "Temp Read Blocks": 0,
                                      "Temp Written Blocks": 0,
                                      "Plans": [
                                        {
                                          "Node Type": "Seq Scan",
                                          "Parent Relationship": "InitPlan",
                                          "Subplan Name": "InitPlan 10",
                                          "Parallel Aware": false,
                                          "Async Capable": false,
                                          "Relation Name": "organizations",
                                          "Alias": "org_1",
                                          "Startup Cost": 0.0,
                                          "Total Cost": 1.2,
                                          "Plan Rows": 1,
                                          "Plan Width": 0,
                                          "Actual Startup Time": 0.004,
                                          "Actual Total Time": 0.004,
                                          "Actual Rows": 1,
                                          "Actual Loops": 10060,
                                          "Filter": "((id = o_1.organization_id) AND ((verification_status)::text = 'APPROVED'::text))",
                                          "Rows Removed by Filter": 3,
                                          "Shared Hit Blocks": 10060,
                                          "Shared Read Blocks": 0,
                                          "Shared Dirtied Blocks": 0,
                                          "Shared Written Blocks": 0,
                                          "Local Hit Blocks": 0,
                                          "Local Read Blocks": 0,
                                          "Local Dirtied Blocks": 0,
                                          "Local Written Blocks": 0,
                                          "Temp Read Blocks": 0,
                                          "Temp Written Blocks": 0
                                        },
                                        {
                                          "Node Type": "Index Scan",
                                          "Parent Relationship": "Outer",
                                          "Parallel Aware": false,
                                          "Async Capable": false,
                                          "Scan Direction": "Forward",
                                          "Index Name": "ix_users_organization_id_id",
                                          "Relation Name": "users",
                                          "Alias": "owner",
                                          "Startup Cost": 1.33,
                                          "Total Cost": 10.5,
                                          "Plan Rows": 1,
                                          "Plan Width": 0,
                                          "Actual Startup Time": 0.005,
                                          "Actual Total Time": 0.005,
                                          "Actual Rows": 1,
                                          "Actual Loops": 10059,
                                          "Index Cond": "((organization_id = o_1.organization_id) AND (id = o_1.owner_user_id))",
                                          "Rows Removed by Index Recheck": 0,
                                          "Filter": "(email_verified AND (NOT must_change_password) AND ((role)::text = ANY ('{BUYER,SUPPLIER}'::text[])) AND ((kyc_status)::text <> 'REJECTED'::text) AND ((status)::text = 'APPROVED'::text) AND ((((kyc_status)::text = 'APPROVED'::text) AND (kyc_organization_id = organization_id)) OR (ANY ((id = (hashed SubPlan 9).col1) AND (organization_id = (hashed SubPlan 9).col2)))))",
                                          "Rows Removed by Filter": 0,
                                          "Shared Hit Blocks": 20118,
                                          "Shared Read Blocks": 0,
                                          "Shared Dirtied Blocks": 0,
                                          "Shared Written Blocks": 0,
                                          "Local Hit Blocks": 0,
                                          "Local Read Blocks": 0,
                                          "Local Dirtied Blocks": 0,
                                          "Local Written Blocks": 0,
                                          "Temp Read Blocks": 0,
                                          "Temp Written Blocks": 0,
                                          "Plans": [
                                            {
                                              "Node Type": "Seq Scan",
                                              "Parent Relationship": "SubPlan",
                                              "Subplan Name": "SubPlan 9",
                                              "Parallel Aware": false,
                                              "Async Capable": false,
                                              "Relation Name": "kyc_execution_grace",
                                              "Alias": "grace_1",
                                              "Startup Cost": 0.0,
                                              "Total Cost": 1.1,
                                              "Plan Rows": 6,
                                              "Plan Width": 32,
                                              "Actual Startup Time": 0.008,
                                              "Actual Total Time": 0.01,
                                              "Actual Rows": 6,
                                              "Actual Loops": 1,
                                              "Filter": "(expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone)",
                                              "Rows Removed by Filter": 2,
                                              "Shared Hit Blocks": 1,
                                              "Shared Read Blocks": 0,
                                              "Shared Dirtied Blocks": 0,
                                              "Shared Written Blocks": 0,
                                              "Local Hit Blocks": 0,
                                              "Local Read Blocks": 0,
                                              "Local Dirtied Blocks": 0,
                                              "Local Written Blocks": 0,
                                              "Temp Read Blocks": 0,
                                              "Temp Written Blocks": 0
                                            }
                                          ]
                                        }
                                      ]
                                    },
                                    {
                                      "Node Type": "CTE Scan",
                                      "Parent Relationship": "SubPlan",
                                      "Subplan Name": "SubPlan 17",
                                      "Parallel Aware": false,
                                      "Async Capable": false,
                                      "CTE Name": "policy_products",
                                      "Alias": "pl",
                                      "Startup Cost": 0.0,
                                      "Total Cost": 2.25,
                                      "Plan Rows": 1,
                                      "Plan Width": 16,
                                      "Actual Startup Time": 0.102,
                                      "Actual Total Time": 0.11,
                                      "Actual Rows": 1,
                                      "Actual Loops": 1,
                                      "Filter": "(qualification_profile = 'lng_v1'::text)",
                                      "Rows Removed by Filter": 5,
                                      "Shared Hit Blocks": 0,
                                      "Shared Read Blocks": 0,
                                      "Shared Dirtied Blocks": 0,
                                      "Shared Written Blocks": 0,
                                      "Local Hit Blocks": 0,
                                      "Local Read Blocks": 0,
                                      "Local Dirtied Blocks": 0,
                                      "Local Written Blocks": 0,
                                      "Temp Read Blocks": 0,
                                      "Temp Written Blocks": 0
                                    },
                                    {
                                      "Node Type": "CTE Scan",
                                      "Parent Relationship": "SubPlan",
                                      "Subplan Name": "SubPlan 19",
                                      "Parallel Aware": false,
                                      "Async Capable": false,
                                      "CTE Name": "policy_products",
                                      "Alias": "pl_1",
                                      "Startup Cost": 0.0,
                                      "Total Cost": 177.75,
                                      "Plan Rows": 1,
                                      "Plan Width": 0,
                                      "Actual Startup Time": 0.006,
                                      "Actual Total Time": 0.006,
                                      "Actual Rows": 1,
                                      "Actual Loops": 4,
                                      "Filter": "((qualification_profile = 'lng_v1'::text) AND (id = o_1.product_id) AND EXISTS(SubPlan 18))",
                                      "Rows Removed by Filter": 0,
                                      "Shared Hit Blocks": 0,
                                      "Shared Read Blocks": 0,
                                      "Shared Dirtied Blocks": 0,
                                      "Shared Written Blocks": 0,
                                      "Local Hit Blocks": 0,
                                      "Local Read Blocks": 0,
                                      "Local Dirtied Blocks": 0,
                                      "Local Written Blocks": 0,
                                      "Temp Read Blocks": 0,
                                      "Temp Written Blocks": 0,
                                      "Plans": [
                                        {
                                          "Node Type": "Function Scan",
                                          "Parent Relationship": "SubPlan",
                                          "Subplan Name": "SubPlan 18",
                                          "Parallel Aware": false,
                                          "Async Capable": false,
                                          "Function Name": "jsonb_array_elements_text",
                                          "Alias": "lane_1",
                                          "Startup Cost": 0.0,
                                          "Total Cost": 1.75,
                                          "Plan Rows": 1,
                                          "Plan Width": 0,
                                          "Actual Startup Time": 0.004,
                                          "Actual Total Time": 0.004,
                                          "Actual Rows": 1,
                                          "Actual Loops": 4,
                                          "Filter": "(value = (o_1.delivery_point_id)::text)",
                                          "Rows Removed by Filter": 0,
                                          "Shared Hit Blocks": 0,
                                          "Shared Read Blocks": 0,
                                          "Shared Dirtied Blocks": 0,
                                          "Shared Written Blocks": 0,
                                          "Local Hit Blocks": 0,
                                          "Local Read Blocks": 0,
                                          "Local Dirtied Blocks": 0,
                                          "Local Written Blocks": 0,
                                          "Temp Read Blocks": 0,
                                          "Temp Written Blocks": 0
                                        }
                                      ]
                                    },
                                    {
                                      "Node Type": "CTE Scan",
                                      "Parent Relationship": "SubPlan",
                                      "Subplan Name": "SubPlan 13",
                                      "Parallel Aware": false,
                                      "Async Capable": false,
                                      "CTE Name": "policy_products",
                                      "Alias": "pf",
                                      "Startup Cost": 0.0,
                                      "Total Cost": 2.25,
                                      "Plan Rows": 1,
                                      "Plan Width": 16,
                                      "Actual Startup Time": 0.005,
                                      "Actual Total Time": 0.006,
                                      "Actual Rows": 1,
                                      "Actual Loops": 1,
                                      "Filter": "(qualification_profile = 'fame_v1'::text)",
                                      "Rows Removed by Filter": 5,
                                      "Shared Hit Blocks": 0,
                                      "Shared Read Blocks": 0,
                                      "Shared Dirtied Blocks": 0,
                                      "Shared Written Blocks": 0,
                                      "Local Hit Blocks": 0,
                                      "Local Read Blocks": 0,
                                      "Local Dirtied Blocks": 0,
                                      "Local Written Blocks": 0,
                                      "Temp Read Blocks": 0,
                                      "Temp Written Blocks": 0
                                    },
                                    {
                                      "Node Type": "CTE Scan",
                                      "Parent Relationship": "SubPlan",
                                      "Subplan Name": "SubPlan 15",
                                      "Parallel Aware": false,
                                      "Async Capable": false,
                                      "CTE Name": "policy_products",
                                      "Alias": "pf_1",
                                      "Startup Cost": 0.0,
                                      "Total Cost": 177.75,
                                      "Plan Rows": 1,
                                      "Plan Width": 0,
                                      "Actual Startup Time": 0.007,
                                      "Actual Total Time": 0.008,
                                      "Actual Rows": 1,
                                      "Actual Loops": 4,
                                      "Filter": "((qualification_profile = 'fame_v1'::text) AND (id = o_1.product_id) AND EXISTS(SubPlan 14))",
                                      "Rows Removed by Filter": 1,
                                      "Shared Hit Blocks": 0,
                                      "Shared Read Blocks": 0,
                                      "Shared Dirtied Blocks": 0,
                                      "Shared Written Blocks": 0,
                                      "Local Hit Blocks": 0,
                                      "Local Read Blocks": 0,
                                      "Local Dirtied Blocks": 0,
                                      "Local Written Blocks": 0,
                                      "Temp Read Blocks": 0,
                                      "Temp Written Blocks": 0,
                                      "Plans": [
                                        {
                                          "Node Type": "Function Scan",
                                          "Parent Relationship": "SubPlan",
                                          "Subplan Name": "SubPlan 14",
                                          "Parallel Aware": false,
                                          "Async Capable": false,
                                          "Function Name": "jsonb_array_elements_text",
                                          "Alias": "lane",
                                          "Startup Cost": 0.0,
                                          "Total Cost": 1.75,
                                          "Plan Rows": 1,
                                          "Plan Width": 0,
                                          "Actual Startup Time": 0.005,
                                          "Actual Total Time": 0.005,
                                          "Actual Rows": 1,
                                          "Actual Loops": 4,
                                          "Filter": "(value = (o_1.delivery_point_id)::text)",
                                          "Rows Removed by Filter": 0,
                                          "Shared Hit Blocks": 0,
                                          "Shared Read Blocks": 0,
                                          "Shared Dirtied Blocks": 0,
                                          "Shared Written Blocks": 0,
                                          "Local Hit Blocks": 0,
                                          "Local Read Blocks": 0,
                                          "Local Dirtied Blocks": 0,
                                          "Local Written Blocks": 0,
                                          "Temp Read Blocks": 0,
                                          "Temp Written Blocks": 0
                                        }
                                      ]
                                    }
                                  ]
                                },
                                {
                                  "Node Type": "Materialize",
                                  "Parent Relationship": "Inner",
                                  "Parallel Aware": false,
                                  "Async Capable": false,
                                  "Startup Cost": 0.0,
                                  "Total Cost": 1.13,
                                  "Plan Rows": 8,
                                  "Plan Width": 31,
                                  "Actual Startup Time": 0.0,
                                  "Actual Total Time": 0.0,
                                  "Actual Rows": 1,
                                  "Actual Loops": 10050,
                                  "Shared Hit Blocks": 1,
                                  "Shared Read Blocks": 0,
                                  "Shared Dirtied Blocks": 0,
                                  "Shared Written Blocks": 0,
                                  "Local Hit Blocks": 0,
                                  "Local Read Blocks": 0,
                                  "Local Dirtied Blocks": 0,
                                  "Local Written Blocks": 0,
                                  "Temp Read Blocks": 0,
                                  "Temp Written Blocks": 0,
                                  "Plans": [
                                    {
                                      "Node Type": "Seq Scan",
                                      "Parent Relationship": "Outer",
                                      "Parallel Aware": false,
                                      "Async Capable": false,
                                      "Relation Name": "delivery_points",
                                      "Alias": "dp",
                                      "Startup Cost": 0.0,
                                      "Total Cost": 1.09,
                                      "Plan Rows": 8,
                                      "Plan Width": 31,
                                      "Actual Startup Time": 0.01,
                                      "Actual Total Time": 0.013,
                                      "Actual Rows": 8,
                                      "Actual Loops": 1,
                                      "Filter": "is_active",
                                      "Rows Removed by Filter": 1,
                                      "Shared Hit Blocks": 1,
                                      "Shared Read Blocks": 0,
                                      "Shared Dirtied Blocks": 0,
                                      "Shared Written Blocks": 0,
                                      "Local Hit Blocks": 0,
                                      "Local Read Blocks": 0,
                                      "Local Dirtied Blocks": 0,
                                      "Local Written Blocks": 0,
                                      "Temp Read Blocks": 0,
                                      "Temp Written Blocks": 0
                                    }
                                  ]
                                }
                              ]
                            },
                            {
                              "Node Type": "Materialize",
                              "Parent Relationship": "Inner",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Startup Cost": 0.0,
                              "Total Cost": 1.14,
                              "Plan Rows": 6,
                              "Plan Width": 41,
                              "Actual Startup Time": 0.0,
                              "Actual Total Time": 0.0,
                              "Actual Rows": 2,
                              "Actual Loops": 10049,
                              "Shared Hit Blocks": 1,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0,
                              "Plans": [
                                {
                                  "Node Type": "Seq Scan",
                                  "Parent Relationship": "Outer",
                                  "Parallel Aware": false,
                                  "Async Capable": false,
                                  "Relation Name": "products",
                                  "Alias": "p",
                                  "Startup Cost": 0.0,
                                  "Total Cost": 1.1,
                                  "Plan Rows": 6,
                                  "Plan Width": 41,
                                  "Actual Startup Time": 0.007,
                                  "Actual Total Time": 0.011,
                                  "Actual Rows": 6,
                                  "Actual Loops": 1,
                                  "Filter": "(is_active AND ((fuel_type)::text = ANY ('{Methanol,Ethanol,FAME,LNG}'::text[])))",
                                  "Rows Removed by Filter": 1,
                                  "Shared Hit Blocks": 1,
                                  "Shared Read Blocks": 0,
                                  "Shared Dirtied Blocks": 0,
                                  "Shared Written Blocks": 0,
                                  "Local Hit Blocks": 0,
                                  "Local Read Blocks": 0,
                                  "Local Dirtied Blocks": 0,
                                  "Local Written Blocks": 0,
                                  "Temp Read Blocks": 0,
                                  "Temp Written Blocks": 0
                                }
                              ]
                            },
                            {
                              "Node Type": "CTE Scan",
                              "Parent Relationship": "SubPlan",
                              "Subplan Name": "SubPlan 21",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "CTE Name": "policy_products",
                              "Alias": "pd",
                              "Startup Cost": 0.0,
                              "Total Cost": 2.25,
                              "Plan Rows": 2,
                              "Plan Width": 112,
                              "Actual Startup Time": 0.002,
                              "Actual Total Time": 0.005,
                              "Actual Rows": 2,
                              "Actual Loops": 1,
                              "Filter": "(qualification_profile = ANY ('{fame_v1,lng_v1}'::text[]))",
                              "Rows Removed by Filter": 4,
                              "Shared Hit Blocks": 0,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            },
                            {
                              "Node Type": "CTE Scan",
                              "Parent Relationship": "SubPlan",
                              "Subplan Name": "SubPlan 23",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "CTE Name": "policy_products",
                              "Alias": "po",
                              "Startup Cost": 0.0,
                              "Total Cost": 2.25,
                              "Plan Rows": 1,
                              "Plan Width": 112,
                              "Actual Startup Time": 0.002,
                              "Actual Total Time": 0.003,
                              "Actual Rows": 1,
                              "Actual Loops": 1,
                              "Filter": "(qualification_profile = 'fame_v1'::text)",
                              "Rows Removed by Filter": 5,
                              "Shared Hit Blocks": 0,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            }
                          ]
                        },
                        {
                          "Node Type": "CTE Scan",
                          "Parent Relationship": "Inner",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "CTE Name": "policy_products",
                          "Alias": "pp",
                          "Startup Cost": 0.0,
                          "Total Cost": 2.25,
                          "Plan Rows": 1,
                          "Plan Width": 144,
                          "Actual Startup Time": 0.001,
                          "Actual Total Time": 0.001,
                          "Actual Rows": 3,
                          "Actual Loops": 10047,
                          "Filter": "(execution_mode = 'ORDERBOOK'::text)",
                          "Rows Removed by Filter": 0,
                          "Shared Hit Blocks": 0,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0
                        },
                        {
                          "Node Type": "Function Scan",
                          "Parent Relationship": "SubPlan",
                          "Subplan Name": "SubPlan 24",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Function Name": "jsonb_array_elements_text",
                          "Alias": "lane_2",
                          "Startup Cost": 0.0,
                          "Total Cost": 1.75,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.006,
                          "Actual Total Time": 0.006,
                          "Actual Rows": 1,
                          "Actual Loops": 10046,
                          "Filter": "(value = (o_1.delivery_point_id)::text)",
                          "Rows Removed by Filter": 3,
                          "Shared Hit Blocks": 0,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0
                        }
                      ]
                    },
                    {
                      "Node Type": "Function Scan",
                      "Parent Relationship": "Inner",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Function Name": "jsonb_to_recordset",
                      "Alias": "policy_point",
                      "Startup Cost": 0.0,
                      "Total Cost": 1.0,
                      "Plan Rows": 100,
                      "Plan Width": 80,
                      "Actual Startup Time": 0.0,
                      "Actual Total Time": 0.001,
                      "Actual Rows": 4,
                      "Actual Loops": 10045,
                      "Shared Hit Blocks": 0,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0
                    },
                    {
                      "Node Type": "CTE Scan",
                      "Parent Relationship": "SubPlan",
                      "Subplan Name": "SubPlan 2",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "CTE Name": "policy_products",
                      "Alias": "pp_1",
                      "Startup Cost": 0.0,
                      "Total Cost": 2.25,
                      "Plan Rows": 1,
                      "Plan Width": 32,
                      "Actual Startup Time": 0.001,
                      "Actual Total Time": 0.001,
                      "Actual Rows": 1,
                      "Actual Loops": 10045,
                      "Filter": "(id = p.id)",
                      "Rows Removed by Filter": 5,
                      "Shared Hit Blocks": 0,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0
                    }
                  ]
                }
              ]
            }
          ]
        }
      ]
    },
    {
      "Node Type": "Seq Scan",
      "Parent Relationship": "Inner",
      "Parallel Aware": false,
      "Async Capable": false,
      "Relation Name": "organizations",
      "Alias": "org",
      "Startup Cost": 0.0,
      "Total Cost": 1.13,
      "Plan Rows": 13,
      "Plan Width": 134,
      "Actual Startup Time": 0.003,
      "Actual Total Time": 0.004,
      "Actual Rows": 4,
      "Actual Loops": 50,
      "Shared Hit Blocks": 50,
      "Shared Read Blocks": 0,
      "Shared Dirtied Blocks": 0,
      "Shared Written Blocks": 0,
      "Local Hit Blocks": 0,
      "Local Read Blocks": 0,
      "Local Dirtied Blocks": 0,
      "Local Written Blocks": 0,
      "Temp Read Blocks": 0,
      "Temp Written Blocks": 0
    }
  ]
}
```

### rust /api/orderbook rows=10000

```sql
WITH eligible_orders AS (-- Shared public-order visibility projection (Task 6).
--
-- This is the complete bound internal relation: every caller value is a parameter, so
-- no route ever concatenates text into SQL. Parameters are
--   $1  attested market policy (jsonb text)
--   $2  captured UTC instant (timestamptz)
--   $3  Singapore date (text, YYYY-MM-DD)
--   $4  include_off_spec (boolean)
--   $5  product_id (uuid, null = any)
--   $6  delivery_point_id (uuid, null = any)
--   $7  side (text, null = any)
--   $8  fuel_type (text, null = any)
--   $9  market_product code (text, null = any)
--   $10 region (text, null = any)
--   $11 availability_window (text, null = any)
--   $12 surface: orderbook, listings, or candidates (text)
--   $13 runtime KYC execution gate enabled (boolean)
-- No pagination here: consumers rank, aggregate and page the shared relation.
--
-- Product identity comes only from the policy. The statement never names a
-- product, a product code, or a product UUID.
WITH policy_products AS (
    SELECT *
    FROM jsonb_to_recordset(($1::jsonb) -> 'products') AS policy_product(
        id uuid,
        code text,
        name text,
        fuel_type text,
        fuel_grade text,
        execution_mode text,
        lanes jsonb,
        qualification_profile text,
        display_order integer
    )
),
policy_points AS (
    SELECT *
    FROM jsonb_to_recordset(($1::jsonb) -> 'points') AS policy_point(
        id uuid,
        name text,
        region text,
        timezone text,
        display_order integer
    )
)
SELECT o.*, p.name AS product_name, p.fuel_type AS product_fuel_type,
       p.fuel_grade AS product_fuel_grade, p.is_active AS product_is_active,
       dp.name AS delivery_point_name, dp.region AS delivery_point_region,
       (SELECT pp.code FROM policy_products pp WHERE pp.id = p.id) AS market_product,
       -- Internal only. Never serialize this object as a public DTO.
       to_jsonb(o) || jsonb_build_object(
           'quantity_mt', o.quantity_mt::text,
           'remaining_quantity_mt', o.remaining_quantity_mt::text,
           'price_per_mt_usd', o.price_per_mt_usd::text,
           'carbon_intensity_gco2_mj', o.carbon_intensity_gco2_mj::text,
           'energy_density_mj_kg', o.energy_density_mj_kg::text
       ) AS private_digest_input
FROM orderbook_orders AS o
JOIN products AS p ON p.id = o.product_id
JOIN delivery_points AS dp ON dp.id = o.delivery_point_id
WHERE o.status IN ('OPEN', 'PARTIALLY_FILLED')
    -- Strict canonical product identity, active row, and approved lane.
    AND p.is_active
    AND EXISTS (
        SELECT 1
        FROM policy_products AS pp
        WHERE pp.execution_mode = 'ORDERBOOK'
          AND pp.id = p.id
          AND pp.name = p.name
          AND pp.fuel_type = p.fuel_type
          AND pp.fuel_grade = p.fuel_grade
          AND EXISTS (
              SELECT 1
              FROM jsonb_array_elements_text(pp.lanes) AS lane(value)
              WHERE lane.value = o.delivery_point_id::text
          )
    )
    -- Strict canonical delivery-point identity and active row.
    AND dp.is_active
    AND EXISTS (
        SELECT 1
        FROM policy_points AS pq
        WHERE pq.id = dp.id AND pq.name = dp.name AND pq.region = dp.region
    )
    -- Canonical availability-window grammar.
    AND ($12::text <> 'orderbook' OR (
        o.availability_window = 'SPOT'
        OR o.availability_window ~ '^[0-9]{4}-(0[1-9]|1[0-2])$'
        OR o.availability_window ~ '^[0-9]{4}-Q[1-4]$'
        OR o.availability_window ~ '^[0-9]{4}-CAL$'
    ))
    AND (-- Shared expiry, owner admission and shallow declaration rule.
-- Requires policy_products and alias o. Binds $2, $3 and $13.
TRUE
    -- Currency: an explicit future expiry, or a null expiry on REAL/UNKNOWN.
    AND (
        o.expires_at > $2::timestamptz
        OR (
            o.expires_at IS NULL
            AND o.provenance IN ('REAL', 'UNKNOWN')
        )
    )
    -- Owner admission. Assisted rows use the narrower administrator rule.
    AND (
        CASE
            WHEN o.creation_method = 'MARKET_SUPPORT' THEN EXISTS (
                SELECT 1
                FROM users AS actor
                WHERE actor.id = o.owner_user_id
                  AND actor.id = o.created_by_actor_user_id
                  AND actor.role = 'ADMIN'
                  AND actor.status = 'APPROVED'
                  AND EXISTS (
                      SELECT 1
                      FROM users AS member
                      WHERE member.organization_id = o.organization_id
                        AND member.role IN ('BUYER', 'SUPPLIER')
                        AND member.status = 'APPROVED'
                        AND member.email_verified
                        AND NOT member.must_change_password
                        AND member.kyc_status <> 'REJECTED'
                        AND CASE WHEN $13::boolean THEN (
                            (member.kyc_status = 'APPROVED'
                             AND member.kyc_organization_id = member.organization_id)
                            OR EXISTS (
                                SELECT 1 FROM kyc_execution_grace AS grace
                                WHERE grace.user_id = member.id
                                  AND grace.organization_id = member.organization_id
                                  AND grace.expires_at > $2::timestamptz
                            )
                        ) ELSE (
                            member.kyc_organization_id IS NULL
                            OR member.kyc_organization_id = member.organization_id
                        ) END
                  )
                  AND EXISTS (
                      SELECT 1
                      FROM organizations AS assisted_org
                      WHERE assisted_org.id = o.organization_id
                        AND assisted_org.verification_status = 'APPROVED'
                        AND assisted_org.provenance = 'REAL'
                  )
            )
            ELSE (
                o.owner_user_id IS NULL
                OR EXISTS (
                    SELECT 1
                    FROM users AS owner
                    WHERE owner.id = o.owner_user_id
                      AND owner.organization_id = o.organization_id
                      AND owner.role IN ('BUYER', 'SUPPLIER')
                      AND owner.status = 'APPROVED'
                      AND owner.email_verified
                      AND NOT owner.must_change_password
                      AND owner.kyc_status <> 'REJECTED'
                      AND CASE WHEN $13::boolean THEN (
                          (owner.kyc_status = 'APPROVED'
                           AND owner.kyc_organization_id = owner.organization_id)
                          OR EXISTS (
                              SELECT 1 FROM kyc_execution_grace AS grace
                              WHERE grace.user_id = owner.id
                                AND grace.organization_id = owner.organization_id
                                AND grace.expires_at > $2::timestamptz
                          )
                      ) ELSE (
                          owner.kyc_organization_id IS NULL
                          OR owner.kyc_organization_id = owner.organization_id
                      ) END
                      AND EXISTS (
                          SELECT 1
                          FROM organizations AS org
                          WHERE org.id = o.organization_id
                            AND org.verification_status = 'APPROVED'
                      )
                )
            )
        END
    )
    -- fame_v1 declaration currency, keyed by policy profile.
    AND (
        NOT EXISTS (
            SELECT 1
            FROM policy_products AS pf
            WHERE pf.qualification_profile = 'fame_v1'
              AND pf.id = o.product_id
        )
        OR (
            EXISTS (
                SELECT 1
                FROM policy_products AS pf
                WHERE pf.qualification_profile = 'fame_v1'
                  AND pf.id = o.product_id
                  AND EXISTS (
                      SELECT 1
                      FROM jsonb_array_elements_text(pf.lanes) AS lane(value)
                      WHERE lane.value = o.delivery_point_id::text
                  )
            )
            AND o.fame_terms ->> 'side' = o.side
            AND o.off_spec = false
            AND (
                o.side <> 'ASK'
                OR (
                    o.fame_terms ->> 'certificate_valid_until' >= $3::text
                    AND o.msds_available
                    AND o.certification_declared
                    AND length(btrim(coalesce(o.certification_scheme, ''))) > 0
                )
            )
        )
    )
    -- lng_v1 declaration currency, keyed by policy profile.
    AND (
        NOT EXISTS (
            SELECT 1
            FROM policy_products AS pl
            WHERE pl.qualification_profile = 'lng_v1'
              AND pl.id = o.product_id
        )
        OR (
            EXISTS (
                SELECT 1
                FROM policy_products AS pl
                WHERE pl.qualification_profile = 'lng_v1'
                  AND pl.id = o.product_id
                  AND EXISTS (
                      SELECT 1
                      FROM jsonb_array_elements_text(pl.lanes) AS lane(value)
                      WHERE lane.value = o.delivery_point_id::text
                  )
            )
            AND o.lng_terms ->> 'side' = o.side
            AND o.off_spec = false
        )
    )
)
    -- Independently labelled public rows.
    AND o.provenance IN ('REAL', 'DEMO')
    -- Admitted fuel types.
    AND p.fuel_type IN ('Methanol', 'Ethanol', 'FAME', 'LNG')
    -- Scalar ASK quality declarations.
    AND ($12::text <> 'orderbook' OR (
        o.side <> 'ASK'
        OR (
            length(btrim(coalesce(o.certification_scheme, ''))) > 0
            AND o.certification_declared
            AND length(btrim(coalesce(o.specification_standard, ''))) > 0
            AND o.msds_available
            AND (
                EXISTS (
                    SELECT 1
                    FROM policy_products AS pd
                    WHERE pd.qualification_profile IN ('fame_v1', 'lng_v1')
                      AND pd.id = p.id
                      AND pd.name = p.name
                      AND pd.fuel_type = p.fuel_type
                      AND pd.fuel_grade = p.fuel_grade
                )
                OR o.carbon_intensity_gco2_mj IS NOT NULL
            )
            AND length(btrim(coalesce(o.feedstock, ''))) > 0
            AND (
                EXISTS (
                    SELECT 1
                    FROM policy_products AS po
                    WHERE po.qualification_profile = 'fame_v1'
                      AND po.id = p.id
                      AND po.name = p.name
                      AND po.fuel_type = p.fuel_type
                      AND po.fuel_grade = p.fuel_grade
                )
                OR length(btrim(coalesce(o.origin, ''))) > 0
            )
        )
    )
    )
    -- Off-spec rows are hidden unless the caller asks for them.
    AND ($12::text <> 'orderbook' OR o.off_spec = false OR $4::boolean)
    AND ($12::text <> 'listings' OR o.side = 'ASK')
    AND $12::text IN ('orderbook', 'listings', 'candidates')
    -- Route filters.
    AND ($5::uuid IS NULL OR o.product_id = $5::uuid)
    AND ($6::uuid IS NULL OR o.delivery_point_id = $6::uuid)
    AND ($7::text IS NULL OR o.side = $7::text)
    AND ($8::text IS NULL OR p.fuel_type = $8::text)
    AND (
        $9::text IS NULL
        OR EXISTS (
            SELECT 1
            FROM policy_products AS pm
            WHERE pm.code = $9::text
              AND pm.id = p.id
              AND pm.name = p.name
              AND pm.fuel_type = p.fuel_type
              AND pm.fuel_grade = p.fuel_grade
        )
    )
    AND ($10::text IS NULL OR $10::text = '' OR dp.region = $10::text OR dp.name = $10::text)
    AND ($11::text IS NULL OR o.availability_window = $11::text)
) , requested_keys AS MATERIALIZED (
    SELECT DISTINCT side, product, point, "window"
    FROM jsonb_to_recordset($14::jsonb) AS requested(
        side text, product text, point uuid, "window" text
    )
)
SELECT o.side, o.market_product, o.delivery_point_id, o.availability_window,
       o.remaining_quantity_mt, o.price_per_mt_usd, o.private_digest_input
FROM eligible_orders AS o
JOIN requested_keys AS requested
    ON (o.side, o.market_product, o.delivery_point_id, o.availability_window)
        = (requested.side, requested.product, requested.point, requested."window")

```

```json
{
  "Node Type": "Nested Loop",
  "Parallel Aware": false,
  "Async Capable": false,
  "Join Type": "Inner",
  "Startup Cost": 4.01,
  "Total Cost": 3897781.65,
  "Plan Rows": 1,
  "Plan Width": 100,
  "Actual Startup Time": 6.104,
  "Actual Total Time": 3465.408,
  "Actual Rows": 10000,
  "Actual Loops": 1,
  "Inner Unique": false,
  "Join Filter": "(((o.side)::text = requested.side) AND (((SubPlan 3)) = requested.product) AND (o.delivery_point_id = requested.point) AND ((o.availability_window)::text = requested.\"window\"))",
  "Rows Removed by Join Filter": 47,
  "Shared Hit Blocks": 30473,
  "Shared Read Blocks": 0,
  "Shared Dirtied Blocks": 0,
  "Shared Written Blocks": 0,
  "Local Hit Blocks": 0,
  "Local Read Blocks": 0,
  "Local Dirtied Blocks": 0,
  "Local Written Blocks": 0,
  "Temp Read Blocks": 0,
  "Temp Written Blocks": 0,
  "Plans": [
    {
      "Node Type": "Aggregate",
      "Strategy": "Hashed",
      "Partial Mode": "Simple",
      "Parent Relationship": "InitPlan",
      "Subplan Name": "CTE requested_keys",
      "Parallel Aware": false,
      "Async Capable": false,
      "Startup Cost": 2.0,
      "Total Cost": 3.0,
      "Plan Rows": 100,
      "Plan Width": 112,
      "Actual Startup Time": 0.298,
      "Actual Total Time": 0.3,
      "Actual Rows": 1,
      "Actual Loops": 1,
      "Group Key": [
        "requested_1.side",
        "requested_1.product",
        "requested_1.point",
        "requested_1.\"window\""
      ],
      "Planned Partitions": 0,
      "HashAgg Batches": 1,
      "Peak Memory Usage": 32,
      "Disk Usage": 0,
      "Shared Hit Blocks": 0,
      "Shared Read Blocks": 0,
      "Shared Dirtied Blocks": 0,
      "Shared Written Blocks": 0,
      "Local Hit Blocks": 0,
      "Local Read Blocks": 0,
      "Local Dirtied Blocks": 0,
      "Local Written Blocks": 0,
      "Temp Read Blocks": 0,
      "Temp Written Blocks": 0,
      "Plans": [
        {
          "Node Type": "Function Scan",
          "Parent Relationship": "Outer",
          "Parallel Aware": false,
          "Async Capable": false,
          "Function Name": "jsonb_to_recordset",
          "Alias": "requested_1",
          "Startup Cost": 0.0,
          "Total Cost": 1.0,
          "Plan Rows": 100,
          "Plan Width": 112,
          "Actual Startup Time": 0.232,
          "Actual Total Time": 0.259,
          "Actual Rows": 50,
          "Actual Loops": 1,
          "Shared Hit Blocks": 0,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0
        }
      ]
    },
    {
      "Node Type": "Nested Loop",
      "Parent Relationship": "Outer",
      "Parallel Aware": false,
      "Async Capable": false,
      "Join Type": "Semi",
      "Startup Cost": 1.0,
      "Total Cost": 3897774.64,
      "Plan Rows": 1,
      "Plan Width": 3541,
      "Actual Startup Time": 0.491,
      "Actual Total Time": 3400.399,
      "Actual Rows": 10047,
      "Actual Loops": 1,
      "Inner Unique": false,
      "Join Filter": "((policy_point.id = dp.id) AND ((dp.name)::text = policy_point.name) AND (policy_point.region = (dp.region)::text))",
      "Rows Removed by Join Filter": 30145,
      "Shared Hit Blocks": 30473,
      "Shared Read Blocks": 0,
      "Shared Dirtied Blocks": 0,
      "Shared Written Blocks": 0,
      "Local Hit Blocks": 0,
      "Local Read Blocks": 0,
      "Local Dirtied Blocks": 0,
      "Local Written Blocks": 0,
      "Temp Read Blocks": 0,
      "Temp Written Blocks": 0,
      "Plans": [
        {
          "Node Type": "Function Scan",
          "Parent Relationship": "InitPlan",
          "Subplan Name": "CTE policy_products",
          "Parallel Aware": false,
          "Async Capable": false,
          "Function Name": "jsonb_to_recordset",
          "Alias": "policy_product",
          "Startup Cost": 0.0,
          "Total Cost": 1.0,
          "Plan Rows": 100,
          "Plan Width": 244,
          "Actual Startup Time": 0.062,
          "Actual Total Time": 0.063,
          "Actual Rows": 6,
          "Actual Loops": 1,
          "Shared Hit Blocks": 0,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0
        },
        {
          "Node Type": "Nested Loop",
          "Parent Relationship": "Outer",
          "Parallel Aware": false,
          "Async Capable": false,
          "Join Type": "Semi",
          "Startup Cost": 0.0,
          "Total Cost": 3897768.6,
          "Plan Rows": 1,
          "Plan Width": 3034,
          "Actual Startup Time": 0.273,
          "Actual Total Time": 758.504,
          "Actual Rows": 10047,
          "Actual Loops": 1,
          "Inner Unique": false,
          "Join Filter": "((pp.id = p.id) AND ((p.name)::text = pp.name) AND (pp.fuel_type = (p.fuel_type)::text) AND (pp.fuel_grade = (p.fuel_grade)::text) AND EXISTS(SubPlan 21))",
          "Rows Removed by Join Filter": 20095,
          "Shared Hit Blocks": 30473,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0,
          "Plans": [
            {
              "Node Type": "Nested Loop",
              "Parent Relationship": "Outer",
              "Parallel Aware": false,
              "Async Capable": false,
              "Join Type": "Inner",
              "Startup Cost": 0.0,
              "Total Cost": 3896049.81,
              "Plan Rows": 963,
              "Plan Width": 3074,
              "Actual Startup Time": 0.254,
              "Actual Total Time": 493.769,
              "Actual Rows": 10049,
              "Actual Loops": 1,
              "Inner Unique": true,
              "Join Filter": "(o.delivery_point_id = dp.id)",
              "Rows Removed by Join Filter": 31,
              "Shared Hit Blocks": 30473,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0,
              "Plans": [
                {
                  "Node Type": "Nested Loop",
                  "Parent Relationship": "Outer",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Join Type": "Inner",
                  "Startup Cost": 0.0,
                  "Total Cost": 3895929.94,
                  "Plan Rows": 1083,
                  "Plan Width": 3043,
                  "Actual Startup Time": 0.237,
                  "Actual Total Time": 428.352,
                  "Actual Rows": 10050,
                  "Actual Loops": 1,
                  "Inner Unique": true,
                  "Join Filter": "(o.product_id = p.id)",
                  "Rows Removed by Join Filter": 10078,
                  "Shared Hit Blocks": 30472,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0,
                  "Plans": [
                    {
                      "Node Type": "Seq Scan",
                      "Parent Relationship": "Outer",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Relation Name": "orderbook_orders",
                      "Alias": "o",
                      "Startup Cost": 0.0,
                      "Total Cost": 3895826.7,
                      "Plan Rows": 1264,
                      "Plan Width": 3003,
                      "Actual Startup Time": 0.212,
                      "Actual Total Time": 413.185,
                      "Actual Rows": 10051,
                      "Actual Loops": 1,
                      "Filter": "(((status)::text = ANY ('{OPEN,PARTIALLY_FILLED}'::text[])) AND ((provenance)::text = ANY ('{REAL,DEMO}'::text[])) AND ((expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone) OR ((expires_at IS NULL) AND ((provenance)::text = ANY ('{REAL,UNKNOWN}'::text[])))) AND CASE WHEN ((creation_method)::text = 'MARKET_SUPPORT'::text) THEN EXISTS(SubPlan 8) ELSE ((owner_user_id IS NULL) OR EXISTS(SubPlan 12)) END AND ((NOT (ANY (product_id = (hashed SubPlan 18).col1))) OR (EXISTS(SubPlan 20) AND ((lng_terms ->> 'side'::text) = (side)::text) AND (NOT off_spec))) AND ((NOT (ANY (product_id = (hashed SubPlan 14).col1))) OR (EXISTS(SubPlan 16) AND ((fame_terms ->> 'side'::text) = (side)::text) AND (NOT off_spec) AND (((side)::text <> 'ASK'::text) OR (((fame_terms ->> 'certificate_valid_until'::text) >= '2026-10-15'::text) AND msds_available AND certification_declared AND (length(btrim((COALESCE(certification_scheme, ''::character varying))::text)) > 0))))))",
                      "Rows Removed by Filter": 28,
                      "Shared Hit Blocks": 30471,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0,
                      "Plans": [
                        {
                          "Node Type": "Result",
                          "Parent Relationship": "SubPlan",
                          "Subplan Name": "SubPlan 8",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Startup Cost": 14.05,
                          "Total Cost": 15.45,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.019,
                          "Actual Total Time": 0.019,
                          "Actual Rows": 0,
                          "Actual Loops": 7,
                          "One-Time Filter": "((InitPlan 6).col1 AND (InitPlan 7).col1 AND (o.owner_user_id = o.created_by_actor_user_id))",
                          "Shared Hit Blocks": 22,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0,
                          "Plans": [
                            {
                              "Node Type": "Index Scan",
                              "Parent Relationship": "InitPlan",
                              "Subplan Name": "InitPlan 6",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Scan Direction": "Forward",
                              "Index Name": "ix_users_organization_id_id",
                              "Relation Name": "users",
                              "Alias": "member",
                              "Startup Cost": 0.14,
                              "Total Cost": 12.82,
                              "Plan Rows": 1,
                              "Plan Width": 0,
                              "Actual Startup Time": 0.011,
                              "Actual Total Time": 0.012,
                              "Actual Rows": 1,
                              "Actual Loops": 7,
                              "Index Cond": "(organization_id = o.organization_id)",
                              "Rows Removed by Index Recheck": 0,
                              "Filter": "(email_verified AND (NOT must_change_password) AND ((role)::text = ANY ('{BUYER,SUPPLIER}'::text[])) AND ((kyc_status)::text <> 'REJECTED'::text) AND ((status)::text = 'APPROVED'::text) AND ((((kyc_status)::text = 'APPROVED'::text) AND (kyc_organization_id = organization_id)) OR (ANY ((id = (hashed SubPlan 5).col1) AND (organization_id = (hashed SubPlan 5).col2)))))",
                              "Rows Removed by Filter": 0,
                              "Shared Hit Blocks": 15,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0,
                              "Plans": [
                                {
                                  "Node Type": "Seq Scan",
                                  "Parent Relationship": "SubPlan",
                                  "Subplan Name": "SubPlan 5",
                                  "Parallel Aware": false,
                                  "Async Capable": false,
                                  "Relation Name": "kyc_execution_grace",
                                  "Alias": "grace",
                                  "Startup Cost": 0.0,
                                  "Total Cost": 1.1,
                                  "Plan Rows": 6,
                                  "Plan Width": 32,
                                  "Actual Startup Time": 0.007,
                                  "Actual Total Time": 0.008,
                                  "Actual Rows": 6,
                                  "Actual Loops": 1,
                                  "Filter": "(expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone)",
                                  "Rows Removed by Filter": 2,
                                  "Shared Hit Blocks": 1,
                                  "Shared Read Blocks": 0,
                                  "Shared Dirtied Blocks": 0,
                                  "Shared Written Blocks": 0,
                                  "Local Hit Blocks": 0,
                                  "Local Read Blocks": 0,
                                  "Local Dirtied Blocks": 0,
                                  "Local Written Blocks": 0,
                                  "Temp Read Blocks": 0,
                                  "Temp Written Blocks": 0
                                }
                              ]
                            },
                            {
                              "Node Type": "Seq Scan",
                              "Parent Relationship": "InitPlan",
                              "Subplan Name": "InitPlan 7",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Relation Name": "organizations",
                              "Alias": "assisted_org",
                              "Startup Cost": 0.0,
                              "Total Cost": 1.23,
                              "Plan Rows": 1,
                              "Plan Width": 0,
                              "Actual Startup Time": 0.004,
                              "Actual Total Time": 0.004,
                              "Actual Rows": 1,
                              "Actual Loops": 4,
                              "Filter": "((id = o.organization_id) AND ((verification_status)::text = 'APPROVED'::text) AND ((provenance)::text = 'REAL'::text))",
                              "Rows Removed by Filter": 4,
                              "Shared Hit Blocks": 4,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            },
                            {
                              "Node Type": "Seq Scan",
                              "Parent Relationship": "Outer",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Relation Name": "users",
                              "Alias": "actor",
                              "Startup Cost": 14.05,
                              "Total Cost": 15.45,
                              "Plan Rows": 1,
                              "Plan Width": 0,
                              "Actual Startup Time": 0.006,
                              "Actual Total Time": 0.006,
                              "Actual Rows": 1,
                              "Actual Loops": 3,
                              "Filter": "((id = o.created_by_actor_user_id) AND ((role)::text = 'ADMIN'::text) AND ((status)::text = 'APPROVED'::text))",
                              "Rows Removed by Filter": 13,
                              "Shared Hit Blocks": 3,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            }
                          ]
                        },
                        {
                          "Node Type": "Result",
                          "Parent Relationship": "SubPlan",
                          "Subplan Name": "SubPlan 12",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Startup Cost": 1.33,
                          "Total Cost": 10.5,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.03,
                          "Actual Total Time": 0.03,
                          "Actual Rows": 1,
                          "Actual Loops": 10061,
                          "One-Time Filter": "(InitPlan 11).col1",
                          "Shared Hit Blocks": 30181,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0,
                          "Plans": [
                            {
                              "Node Type": "Seq Scan",
                              "Parent Relationship": "InitPlan",
                              "Subplan Name": "InitPlan 11",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Relation Name": "organizations",
                              "Alias": "org",
                              "Startup Cost": 0.0,
                              "Total Cost": 1.2,
                              "Plan Rows": 1,
                              "Plan Width": 0,
                              "Actual Startup Time": 0.01,
                              "Actual Total Time": 0.01,
                              "Actual Rows": 1,
                              "Actual Loops": 10061,
                              "Filter": "((id = o.organization_id) AND ((verification_status)::text = 'APPROVED'::text))",
                              "Rows Removed by Filter": 3,
                              "Shared Hit Blocks": 10061,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            },
                            {
                              "Node Type": "Index Scan",
                              "Parent Relationship": "Outer",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Scan Direction": "Forward",
                              "Index Name": "ix_users_organization_id_id",
                              "Relation Name": "users",
                              "Alias": "owner",
                              "Startup Cost": 1.33,
                              "Total Cost": 10.5,
                              "Plan Rows": 1,
                              "Plan Width": 0,
                              "Actual Startup Time": 0.013,
                              "Actual Total Time": 0.013,
                              "Actual Rows": 1,
                              "Actual Loops": 10060,
                              "Index Cond": "((organization_id = o.organization_id) AND (id = o.owner_user_id))",
                              "Rows Removed by Index Recheck": 0,
                              "Filter": "(email_verified AND (NOT must_change_password) AND ((role)::text = ANY ('{BUYER,SUPPLIER}'::text[])) AND ((kyc_status)::text <> 'REJECTED'::text) AND ((status)::text = 'APPROVED'::text) AND ((((kyc_status)::text = 'APPROVED'::text) AND (kyc_organization_id = organization_id)) OR (ANY ((id = (hashed SubPlan 10).col1) AND (organization_id = (hashed SubPlan 10).col2)))))",
                              "Rows Removed by Filter": 0,
                              "Shared Hit Blocks": 20120,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0,
                              "Plans": [
                                {
                                  "Node Type": "Seq Scan",
                                  "Parent Relationship": "SubPlan",
                                  "Subplan Name": "SubPlan 10",
                                  "Parallel Aware": false,
                                  "Async Capable": false,
                                  "Relation Name": "kyc_execution_grace",
                                  "Alias": "grace_1",
                                  "Startup Cost": 0.0,
                                  "Total Cost": 1.1,
                                  "Plan Rows": 6,
                                  "Plan Width": 32,
                                  "Actual Startup Time": 0.006,
                                  "Actual Total Time": 0.008,
                                  "Actual Rows": 6,
                                  "Actual Loops": 1,
                                  "Filter": "(expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone)",
                                  "Rows Removed by Filter": 2,
                                  "Shared Hit Blocks": 1,
                                  "Shared Read Blocks": 0,
                                  "Shared Dirtied Blocks": 0,
                                  "Shared Written Blocks": 0,
                                  "Local Hit Blocks": 0,
                                  "Local Read Blocks": 0,
                                  "Local Dirtied Blocks": 0,
                                  "Local Written Blocks": 0,
                                  "Temp Read Blocks": 0,
                                  "Temp Written Blocks": 0
                                }
                              ]
                            }
                          ]
                        },
                        {
                          "Node Type": "CTE Scan",
                          "Parent Relationship": "SubPlan",
                          "Subplan Name": "SubPlan 18",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "CTE Name": "policy_products",
                          "Alias": "pl",
                          "Startup Cost": 0.0,
                          "Total Cost": 2.25,
                          "Plan Rows": 1,
                          "Plan Width": 16,
                          "Actual Startup Time": 0.065,
                          "Actual Total Time": 0.073,
                          "Actual Rows": 1,
                          "Actual Loops": 1,
                          "Filter": "(qualification_profile = 'lng_v1'::text)",
                          "Rows Removed by Filter": 5,
                          "Shared Hit Blocks": 0,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0
                        },
                        {
                          "Node Type": "CTE Scan",
                          "Parent Relationship": "SubPlan",
                          "Subplan Name": "SubPlan 20",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "CTE Name": "policy_products",
                          "Alias": "pl_1",
                          "Startup Cost": 0.0,
                          "Total Cost": 177.75,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.049,
                          "Actual Total Time": 0.049,
                          "Actual Rows": 1,
                          "Actual Loops": 4,
                          "Filter": "((qualification_profile = 'lng_v1'::text) AND (id = o.product_id) AND EXISTS(SubPlan 19))",
                          "Rows Removed by Filter": 0,
                          "Shared Hit Blocks": 0,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0,
                          "Plans": [
                            {
                              "Node Type": "Function Scan",
                              "Parent Relationship": "SubPlan",
                              "Subplan Name": "SubPlan 19",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Function Name": "jsonb_array_elements_text",
                              "Alias": "lane_1",
                              "Startup Cost": 0.0,
                              "Total Cost": 1.75,
                              "Plan Rows": 1,
                              "Plan Width": 0,
                              "Actual Startup Time": 0.034,
                              "Actual Total Time": 0.035,
                              "Actual Rows": 1,
                              "Actual Loops": 4,
                              "Filter": "(value = (o.delivery_point_id)::text)",
                              "Rows Removed by Filter": 0,
                              "Shared Hit Blocks": 0,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            }
                          ]
                        },
                        {
                          "Node Type": "CTE Scan",
                          "Parent Relationship": "SubPlan",
                          "Subplan Name": "SubPlan 14",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "CTE Name": "policy_products",
                          "Alias": "pf",
                          "Startup Cost": 0.0,
                          "Total Cost": 2.25,
                          "Plan Rows": 1,
                          "Plan Width": 16,
                          "Actual Startup Time": 0.001,
                          "Actual Total Time": 0.003,
                          "Actual Rows": 1,
                          "Actual Loops": 1,
                          "Filter": "(qualification_profile = 'fame_v1'::text)",
                          "Rows Removed by Filter": 5,
                          "Shared Hit Blocks": 0,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0
                        },
                        {
                          "Node Type": "CTE Scan",
                          "Parent Relationship": "SubPlan",
                          "Subplan Name": "SubPlan 16",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "CTE Name": "policy_products",
                          "Alias": "pf_1",
                          "Startup Cost": 0.0,
                          "Total Cost": 177.75,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.006,
                          "Actual Total Time": 0.006,
                          "Actual Rows": 1,
                          "Actual Loops": 4,
                          "Filter": "((qualification_profile = 'fame_v1'::text) AND (id = o.product_id) AND EXISTS(SubPlan 15))",
                          "Rows Removed by Filter": 1,
                          "Shared Hit Blocks": 0,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0,
                          "Plans": [
                            {
                              "Node Type": "Function Scan",
                              "Parent Relationship": "SubPlan",
                              "Subplan Name": "SubPlan 15",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Function Name": "jsonb_array_elements_text",
                              "Alias": "lane",
                              "Startup Cost": 0.0,
                              "Total Cost": 1.75,
                              "Plan Rows": 1,
                              "Plan Width": 0,
                              "Actual Startup Time": 0.004,
                              "Actual Total Time": 0.004,
                              "Actual Rows": 1,
                              "Actual Loops": 4,
                              "Filter": "(value = (o.delivery_point_id)::text)",
                              "Rows Removed by Filter": 0,
                              "Shared Hit Blocks": 0,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            }
                          ]
                        }
                      ]
                    },
                    {
                      "Node Type": "Materialize",
                      "Parent Relationship": "Inner",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Startup Cost": 0.0,
                      "Total Cost": 1.14,
                      "Plan Rows": 6,
                      "Plan Width": 40,
                      "Actual Startup Time": 0.0,
                      "Actual Total Time": 0.0,
                      "Actual Rows": 2,
                      "Actual Loops": 10051,
                      "Shared Hit Blocks": 1,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0,
                      "Plans": [
                        {
                          "Node Type": "Seq Scan",
                          "Parent Relationship": "Outer",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Relation Name": "products",
                          "Alias": "p",
                          "Startup Cost": 0.0,
                          "Total Cost": 1.1,
                          "Plan Rows": 6,
                          "Plan Width": 40,
                          "Actual Startup Time": 0.01,
                          "Actual Total Time": 0.017,
                          "Actual Rows": 6,
                          "Actual Loops": 1,
                          "Filter": "(is_active AND ((fuel_type)::text = ANY ('{Methanol,Ethanol,FAME,LNG}'::text[])))",
                          "Rows Removed by Filter": 1,
                          "Shared Hit Blocks": 1,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0
                        }
                      ]
                    }
                  ]
                },
                {
                  "Node Type": "Materialize",
                  "Parent Relationship": "Inner",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Startup Cost": 0.0,
                  "Total Cost": 1.13,
                  "Plan Rows": 8,
                  "Plan Width": 31,
                  "Actual Startup Time": 0.0,
                  "Actual Total Time": 0.0,
                  "Actual Rows": 1,
                  "Actual Loops": 10050,
                  "Shared Hit Blocks": 1,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0,
                  "Plans": [
                    {
                      "Node Type": "Seq Scan",
                      "Parent Relationship": "Outer",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Relation Name": "delivery_points",
                      "Alias": "dp",
                      "Startup Cost": 0.0,
                      "Total Cost": 1.09,
                      "Plan Rows": 8,
                      "Plan Width": 31,
                      "Actual Startup Time": 0.006,
                      "Actual Total Time": 0.011,
                      "Actual Rows": 8,
                      "Actual Loops": 1,
                      "Filter": "is_active",
                      "Rows Removed by Filter": 1,
                      "Shared Hit Blocks": 1,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0
                    }
                  ]
                }
              ]
            },
            {
              "Node Type": "CTE Scan",
              "Parent Relationship": "Inner",
              "Parallel Aware": false,
              "Async Capable": false,
              "CTE Name": "policy_products",
              "Alias": "pp",
              "Startup Cost": 0.0,
              "Total Cost": 2.25,
              "Plan Rows": 1,
              "Plan Width": 144,
              "Actual Startup Time": 0.004,
              "Actual Total Time": 0.007,
              "Actual Rows": 3,
              "Actual Loops": 10049,
              "Filter": "(execution_mode = 'ORDERBOOK'::text)",
              "Rows Removed by Filter": 0,
              "Shared Hit Blocks": 0,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0
            },
            {
              "Node Type": "Function Scan",
              "Parent Relationship": "SubPlan",
              "Subplan Name": "SubPlan 21",
              "Parallel Aware": false,
              "Async Capable": false,
              "Function Name": "jsonb_array_elements_text",
              "Alias": "lane_2",
              "Startup Cost": 0.0,
              "Total Cost": 1.75,
              "Plan Rows": 1,
              "Plan Width": 0,
              "Actual Startup Time": 0.013,
              "Actual Total Time": 0.013,
              "Actual Rows": 1,
              "Actual Loops": 10048,
              "Filter": "(value = (o.delivery_point_id)::text)",
              "Rows Removed by Filter": 3,
              "Shared Hit Blocks": 0,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0
            }
          ]
        },
        {
          "Node Type": "Function Scan",
          "Parent Relationship": "Inner",
          "Parallel Aware": false,
          "Async Capable": false,
          "Function Name": "jsonb_to_recordset",
          "Alias": "policy_point",
          "Startup Cost": 0.0,
          "Total Cost": 1.0,
          "Plan Rows": 100,
          "Plan Width": 80,
          "Actual Startup Time": 0.0,
          "Actual Total Time": 0.002,
          "Actual Rows": 4,
          "Actual Loops": 10047,
          "Shared Hit Blocks": 0,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0
        },
        {
          "Node Type": "CTE Scan",
          "Parent Relationship": "SubPlan",
          "Subplan Name": "SubPlan 3",
          "Parallel Aware": false,
          "Async Capable": false,
          "CTE Name": "policy_products",
          "Alias": "pp_1",
          "Startup Cost": 0.0,
          "Total Cost": 2.25,
          "Plan Rows": 1,
          "Plan Width": 32,
          "Actual Startup Time": 0.001,
          "Actual Total Time": 0.004,
          "Actual Rows": 1,
          "Actual Loops": 10047,
          "Filter": "(id = p.id)",
          "Rows Removed by Filter": 5,
          "Shared Hit Blocks": 0,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0
        }
      ]
    },
    {
      "Node Type": "CTE Scan",
      "Parent Relationship": "Inner",
      "Parallel Aware": false,
      "Async Capable": false,
      "CTE Name": "requested_keys",
      "Alias": "requested",
      "Startup Cost": 0.0,
      "Total Cost": 2.0,
      "Plan Rows": 100,
      "Plan Width": 112,
      "Actual Startup Time": 0.0,
      "Actual Total Time": 0.001,
      "Actual Rows": 1,
      "Actual Loops": 10047,
      "Shared Hit Blocks": 0,
      "Shared Read Blocks": 0,
      "Shared Dirtied Blocks": 0,
      "Shared Written Blocks": 0,
      "Local Hit Blocks": 0,
      "Local Read Blocks": 0,
      "Local Dirtied Blocks": 0,
      "Local Written Blocks": 0,
      "Temp Read Blocks": 0,
      "Temp Written Blocks": 0
    }
  ]
}
```

### python /api/orderbook/bids rows=1

```sql
SELECT count(orderbook_orders.id) AS count_1 
FROM orderbook_orders JOIN products ON orderbook_orders.product_id = products.id JOIN delivery_points ON orderbook_orders.delivery_point_id = delivery_points.id 
WHERE orderbook_orders.status IN ($131, $132) AND orderbook_orders.side = $1 AND products.is_active IS true AND (products.id = $2::UUID AND products.name = $3::VARCHAR AND products.fuel_type = $4::VARCHAR AND products.fuel_grade = $5::VARCHAR OR products.id = $6::UUID AND products.name = $7::VARCHAR AND products.fuel_type = $8::VARCHAR AND products.fuel_grade = $9::VARCHAR OR products.id = $10::UUID AND products.name = $11::VARCHAR AND products.fuel_type = $12::VARCHAR AND products.fuel_grade = $13::VARCHAR OR products.id = $14::UUID AND products.name = $15::VARCHAR AND products.fuel_type = $16::VARCHAR AND products.fuel_grade = $17::VARCHAR OR products.id = $18::UUID AND products.name = $19::VARCHAR AND products.fuel_type = $20::VARCHAR AND products.fuel_grade = $21::VARCHAR OR products.id = $22::UUID AND products.name = $23::VARCHAR AND products.fuel_type = $24::VARCHAR AND products.fuel_grade = $25::VARCHAR) AND CASE WHEN (products.id = $26::UUID AND products.name = $27::VARCHAR AND products.fuel_type = $28::VARCHAR AND products.fuel_grade = $29::VARCHAR) THEN $30::VARCHAR WHEN (products.id = $31::UUID AND products.name = $32::VARCHAR AND products.fuel_type = $33::VARCHAR AND products.fuel_grade = $34::VARCHAR) THEN $35::VARCHAR WHEN (products.id = $36::UUID AND products.name = $37::VARCHAR AND products.fuel_type = $38::VARCHAR AND products.fuel_grade = $39::VARCHAR) THEN $40::VARCHAR WHEN (products.id = $41::UUID AND products.name = $42::VARCHAR AND products.fuel_type = $43::VARCHAR AND products.fuel_grade = $44::VARCHAR) THEN $45::VARCHAR WHEN (products.id = $46::UUID AND products.name = $47::VARCHAR AND products.fuel_type = $48::VARCHAR AND products.fuel_grade = $49::VARCHAR) THEN $50::VARCHAR WHEN (products.id = $51::UUID AND products.name = $52::VARCHAR AND products.fuel_type = $53::VARCHAR AND products.fuel_grade = $54::VARCHAR) THEN $55::VARCHAR END IN ($133::VARCHAR, $134::VARCHAR, $135::VARCHAR, $136::VARCHAR, $137::VARCHAR, $138::VARCHAR) AND delivery_points.is_active IS true AND (delivery_points.id = $56::UUID AND delivery_points.name = $57::VARCHAR AND delivery_points.region = $58::VARCHAR OR delivery_points.id = $59::UUID AND delivery_points.name = $60::VARCHAR AND delivery_points.region = $61::VARCHAR OR delivery_points.id = $62::UUID AND delivery_points.name = $63::VARCHAR AND delivery_points.region = $64::VARCHAR OR delivery_points.id = $65::UUID AND delivery_points.name = $66::VARCHAR AND delivery_points.region = $67::VARCHAR OR delivery_points.id = $68::UUID AND delivery_points.name = $69::VARCHAR AND delivery_points.region = $70::VARCHAR OR delivery_points.id = $71::UUID AND delivery_points.name = $72::VARCHAR AND delivery_points.region = $73::VARCHAR OR delivery_points.id = $74::UUID AND delivery_points.name = $75::VARCHAR AND delivery_points.region = $76::VARCHAR OR delivery_points.id = $77::UUID AND delivery_points.name = $78::VARCHAR AND delivery_points.region = $79::VARCHAR) AND (products.id = $80::UUID AND delivery_points.id IN ($139::UUID) OR products.id = $81::UUID AND delivery_points.id IN ($140::UUID) OR products.id = $82::UUID OR products.id = $83::UUID OR products.id = $84::UUID OR products.id = $85::UUID) AND (orderbook_orders.availability_window = $86::VARCHAR OR orderbook_orders.availability_window ~ $87::VARCHAR OR orderbook_orders.availability_window ~ $88::VARCHAR OR orderbook_orders.availability_window ~ $89::VARCHAR) AND (orderbook_orders.expires_at > $90::TIMESTAMP WITH TIME ZONE OR orderbook_orders.expires_at IS NULL AND orderbook_orders.provenance IN ($141, $142)) AND CASE WHEN (orderbook_orders.creation_method = $91) THEN EXISTS (SELECT users.id 
FROM users 
WHERE orderbook_orders.creation_method = $92 AND users.id = orderbook_orders.owner_user_id AND users.id = orderbook_orders.created_by_actor_user_id AND users.role = $93 AND users.status = $94 AND (EXISTS (SELECT users_1.id 
FROM users AS users_1 
WHERE users_1.organization_id = orderbook_orders.organization_id AND users_1.role IN ($143, $144) AND users_1.status = $95 AND users_1.email_verified IS true AND users_1.must_change_password IS false AND users_1.kyc_status != $96::VARCHAR AND (users_1.kyc_status = $97::VARCHAR AND users_1.kyc_organization_id = users_1.organization_id OR (EXISTS (SELECT kyc_execution_grace.id 
FROM kyc_execution_grace 
WHERE kyc_execution_grace.user_id = users_1.id AND kyc_execution_grace.organization_id = users_1.organization_id AND kyc_execution_grace.expires_at > $98::TIMESTAMP WITH TIME ZONE))))) AND (EXISTS (SELECT organizations.id 
FROM organizations 
WHERE organizations.id = orderbook_orders.organization_id AND organizations.verification_status = $99::VARCHAR AND organizations.provenance = $100))) ELSE orderbook_orders.owner_user_id IS NULL OR (EXISTS (SELECT users.id 
FROM users 
WHERE users.id = orderbook_orders.owner_user_id AND users.organization_id = orderbook_orders.organization_id AND users.role IN ($145, $146) AND users.status = $101 AND users.email_verified IS true AND users.must_change_password IS false AND users.kyc_status != $102::VARCHAR AND (users.kyc_status = $103::VARCHAR AND users.kyc_organization_id = users.organization_id OR (EXISTS (SELECT kyc_execution_grace.id 
FROM kyc_execution_grace 
WHERE kyc_execution_grace.user_id = users.id AND kyc_execution_grace.organization_id = users.organization_id AND kyc_execution_grace.expires_at > $104::TIMESTAMP WITH TIME ZONE))) AND (EXISTS (SELECT organizations.id 
FROM organizations 
WHERE organizations.id = orderbook_orders.organization_id AND organizations.verification_status = $105::VARCHAR)))) END AND (orderbook_orders.product_id != $106::UUID OR orderbook_orders.delivery_point_id IN ($147::UUID) AND CAST((orderbook_orders.fame_terms ->> $107::TEXT) AS VARCHAR) = CAST(orderbook_orders.side AS VARCHAR) AND orderbook_orders.off_spec IS false AND (orderbook_orders.side != $108 OR CAST((orderbook_orders.fame_terms ->> $109::TEXT) AS VARCHAR) >= $110::VARCHAR AND orderbook_orders.msds_available IS true AND orderbook_orders.certification_declared IS true AND length(trim(coalesce(orderbook_orders.certification_scheme, $111::VARCHAR))) > $112::INTEGER)) AND (orderbook_orders.product_id != $113::UUID OR orderbook_orders.delivery_point_id IN ($148::UUID) AND CAST((orderbook_orders.lng_terms ->> $114::TEXT) AS VARCHAR) = CAST(orderbook_orders.side AS VARCHAR) AND orderbook_orders.off_spec IS false) AND orderbook_orders.provenance IN ($149, $150) AND products.fuel_type IN ($151::VARCHAR, $152::VARCHAR, $153::VARCHAR, $154::VARCHAR) AND orderbook_orders.off_spec IS false AND (orderbook_orders.side != $115 OR length(trim(coalesce(orderbook_orders.certification_scheme, $116::VARCHAR))) > $117::INTEGER) AND (orderbook_orders.side != $118 OR orderbook_orders.certification_declared IS true) AND (orderbook_orders.side != $119 OR length(trim(coalesce(orderbook_orders.specification_standard, $120::VARCHAR))) > $121::INTEGER) AND (orderbook_orders.side != $122 OR orderbook_orders.msds_available IS true) AND (orderbook_orders.side != $123 OR orderbook_orders.product_id IN ($155::UUID, $156::UUID) OR orderbook_orders.carbon_intensity_gco2_mj IS NOT NULL) AND (orderbook_orders.side != $124 OR length(trim(coalesce(orderbook_orders.feedstock, $125::VARCHAR))) > $126::INTEGER) AND (orderbook_orders.side != $127 OR orderbook_orders.product_id = $128::UUID OR length(trim(coalesce(orderbook_orders.origin, $129::VARCHAR))) > $130::INTEGER)
```

```json
{
  "Node Type": "Aggregate",
  "Strategy": "Plain",
  "Partial Mode": "Simple",
  "Parallel Aware": false,
  "Async Capable": false,
  "Startup Cost": 13398.8,
  "Total Cost": 13398.81,
  "Plan Rows": 1,
  "Plan Width": 8,
  "Actual Startup Time": 102.806,
  "Actual Total Time": 102.813,
  "Actual Rows": 1,
  "Actual Loops": 1,
  "Shared Hit Blocks": 30496,
  "Shared Read Blocks": 0,
  "Shared Dirtied Blocks": 0,
  "Shared Written Blocks": 0,
  "Local Hit Blocks": 0,
  "Local Read Blocks": 0,
  "Local Dirtied Blocks": 0,
  "Local Written Blocks": 0,
  "Temp Read Blocks": 0,
  "Temp Written Blocks": 0,
  "Plans": [
    {
      "Node Type": "Nested Loop",
      "Parent Relationship": "Outer",
      "Parallel Aware": false,
      "Async Capable": false,
      "Join Type": "Inner",
      "Startup Cost": 26.02,
      "Total Cost": 13398.64,
      "Plan Rows": 62,
      "Plan Width": 16,
      "Actual Startup Time": 0.702,
      "Actual Total Time": 100.737,
      "Actual Rows": 10023,
      "Actual Loops": 1,
      "Inner Unique": false,
      "Shared Hit Blocks": 30496,
      "Shared Read Blocks": 0,
      "Shared Dirtied Blocks": 0,
      "Shared Written Blocks": 0,
      "Local Hit Blocks": 0,
      "Local Read Blocks": 0,
      "Local Dirtied Blocks": 0,
      "Local Written Blocks": 0,
      "Temp Read Blocks": 0,
      "Temp Written Blocks": 0,
      "Plans": [
        {
          "Node Type": "Nested Loop",
          "Parent Relationship": "Outer",
          "Parallel Aware": false,
          "Async Capable": false,
          "Join Type": "Inner",
          "Startup Cost": 0.0,
          "Total Cost": 3.76,
          "Plan Rows": 1,
          "Plan Width": 32,
          "Actual Startup Time": 0.035,
          "Actual Total Time": 0.106,
          "Actual Rows": 23,
          "Actual Loops": 1,
          "Inner Unique": false,
          "Join Filter": "(((products.id = '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid) AND (delivery_points.id = '1379d36c-1ca9-55b7-9c0d-5235a0ba1f36'::uuid)) OR ((products.id = 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) AND (delivery_points.id = '73835e92-820e-584b-8280-bb61c63aa28e'::uuid)) OR (products.id = '9510c713-6e39-5080-add3-0a7c29657b79'::uuid) OR (products.id = 'f9b20492-b445-59cd-b292-a386d913f488'::uuid) OR (products.id = 'c4a688be-f7c2-5edc-8f93-6b34e387609c'::uuid) OR (products.id = 'd186bffb-766d-5944-8825-989abbdcfc46'::uuid))",
          "Rows Removed by Join Filter": 12,
          "Shared Hit Blocks": 6,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0,
          "Plans": [
            {
              "Node Type": "Seq Scan",
              "Parent Relationship": "Outer",
              "Parallel Aware": false,
              "Async Capable": false,
              "Relation Name": "products",
              "Alias": "products",
              "Startup Cost": 0.0,
              "Total Cost": 2.1,
              "Plan Rows": 1,
              "Plan Width": 16,
              "Actual Startup Time": 0.027,
              "Actual Total Time": 0.044,
              "Actual Rows": 5,
              "Actual Loops": 1,
              "Filter": "((is_active IS TRUE) AND ((fuel_type)::text = ANY ('{Methanol,Ethanol,FAME,LNG}'::text[])) AND ((id = '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid) OR (id = 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) OR (id = '9510c713-6e39-5080-add3-0a7c29657b79'::uuid) OR (id = 'f9b20492-b445-59cd-b292-a386d913f488'::uuid) OR (id = 'c4a688be-f7c2-5edc-8f93-6b34e387609c'::uuid) OR (id = 'd186bffb-766d-5944-8825-989abbdcfc46'::uuid)) AND (((id = '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid) AND ((name)::text = 'Bio-LNG'::text) AND ((fuel_type)::text = 'LNG'::text) AND ((fuel_grade)::text = 'Bio'::text)) OR ((id = 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) AND ((name)::text = 'UCOME B100'::text) AND ((fuel_type)::text = 'FAME'::text) AND ((fuel_grade)::text = 'UCOME'::text)) OR ((id = '9510c713-6e39-5080-add3-0a7c29657b79'::uuid) AND ((name)::text = 'Bio Methanol'::text) AND ((fuel_type)::text = 'Methanol'::text) AND ((fuel_grade)::text = 'Bio'::text)) OR ((id = 'f9b20492-b445-59cd-b292-a386d913f488'::uuid) AND ((name)::text = 'e-Methanol'::text) AND ((fuel_type)::text = 'Methanol'::text) AND ((fuel_grade)::text = 'E'::text)) OR ((id = 'c4a688be-f7c2-5edc-8f93-6b34e387609c'::uuid) AND ((name)::text = 'Bio Ethanol'::text) AND ((fuel_type)::text = 'Ethanol'::text) AND ((fuel_grade)::text = 'Bio'::text)) OR ((id = 'd186bffb-766d-5944-8825-989abbdcfc46'::uuid) AND ((name)::text = 'Synthetic Ethanol'::text) AND ((fuel_type)::text = 'Ethanol'::text) AND ((fuel_grade)::text = 'Synthetic'::text))) AND ((CASE WHEN ((id = '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid) AND ((name)::text = 'Bio-LNG'::text) AND ((fuel_type)::text = 'LNG'::text) AND ((fuel_grade)::text = 'Bio'::text)) THEN 'BIO_LNG'::character varying WHEN ((id = 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) AND ((name)::text = 'UCOME B100'::text) AND ((fuel_type)::text = 'FAME'::text) AND ((fuel_grade)::text = 'UCOME'::text)) THEN 'UCOME_B100'::character varying WHEN ((id = '9510c713-6e39-5080-add3-0a7c29657b79'::uuid) AND ((name)::text = 'Bio Methanol'::text) AND ((fuel_type)::text = 'Methanol'::text) AND ((fuel_grade)::text = 'Bio'::text)) THEN 'BIO_METHANOL'::character varying WHEN ((id = 'f9b20492-b445-59cd-b292-a386d913f488'::uuid) AND ((name)::text = 'e-Methanol'::text) AND ((fuel_type)::text = 'Methanol'::text) AND ((fuel_grade)::text = 'E'::text)) THEN 'E_METHANOL'::character varying WHEN ((id = 'c4a688be-f7c2-5edc-8f93-6b34e387609c'::uuid) AND ((name)::text = 'Bio Ethanol'::text) AND ((fuel_type)::text = 'Ethanol'::text) AND ((fuel_grade)::text = 'Bio'::text)) THEN 'BIO_ETHANOL'::character varying WHEN ((id = 'd186bffb-766d-5944-8825-989abbdcfc46'::uuid) AND ((name)::text = 'Synthetic Ethanol'::text) AND ((fuel_type)::text = 'Ethanol'::text) AND ((fuel_grade)::text = 'Synthetic'::text)) THEN 'SYNTHETIC_ETHANOL'::character varying ELSE NULL::character varying END)::text = ANY ('{BIO_LNG,UCOME_B100,BIO_METHANOL,E_METHANOL,BIO_ETHANOL,SYNTHETIC_ETHANOL}'::text[])))",
              "Rows Removed by Filter": 2,
              "Shared Hit Blocks": 1,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0
            },
            {
              "Node Type": "Seq Scan",
              "Parent Relationship": "Inner",
              "Parallel Aware": false,
              "Async Capable": false,
              "Relation Name": "delivery_points",
              "Alias": "delivery_points",
              "Startup Cost": 0.0,
              "Total Cost": 1.63,
              "Plan Rows": 1,
              "Plan Width": 16,
              "Actual Startup Time": 0.003,
              "Actual Total Time": 0.008,
              "Actual Rows": 7,
              "Actual Loops": 5,
              "Filter": "((is_active IS TRUE) AND (((id = '78281fb3-e726-5396-802e-34b01fda21e8'::uuid) AND ((name)::text = 'Dalian'::text) AND ((region)::text = 'Asia'::text)) OR ((id = '262d36ae-6f35-5785-b9e8-9e221b0f1b78'::uuid) AND ((name)::text = 'Busan'::text) AND ((region)::text = 'Asia'::text)) OR ((id = '633c0593-f9b9-52ad-abe5-a0285f6888d3'::uuid) AND ((name)::text = 'Shanghai'::text) AND ((region)::text = 'Asia'::text)) OR ((id = '73835e92-820e-584b-8280-bb61c63aa28e'::uuid) AND ((name)::text = 'Singapore'::text) AND ((region)::text = 'Asia'::text)) OR ((id = '1379d36c-1ca9-55b7-9c0d-5235a0ba1f36'::uuid) AND ((name)::text = 'Rotterdam'::text) AND ((region)::text = 'Europe'::text)) OR ((id = 'a083db06-b050-56c2-a274-3897eac2fdae'::uuid) AND ((name)::text = 'Houston'::text) AND ((region)::text = 'Americas'::text)) OR ((id = '1be2a5cb-fc34-5e88-8b15-7f7d0fd6e534'::uuid) AND ((name)::text = 'Los Angeles'::text) AND ((region)::text = 'Americas'::text)) OR ((id = 'a4762444-4e0b-5bc1-b647-a185047adae5'::uuid) AND ((name)::text = 'Santos'::text) AND ((region)::text = 'Americas'::text))))",
              "Rows Removed by Filter": 2,
              "Shared Hit Blocks": 5,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0
            }
          ]
        },
        {
          "Node Type": "Bitmap Heap Scan",
          "Parent Relationship": "Inner",
          "Parallel Aware": false,
          "Async Capable": false,
          "Relation Name": "orderbook_orders",
          "Alias": "orderbook_orders",
          "Startup Cost": 26.02,
          "Total Cost": 13392.42,
          "Plan Rows": 246,
          "Plan Width": 48,
          "Actual Startup Time": 0.037,
          "Actual Total Time": 4.286,
          "Actual Rows": 436,
          "Actual Loops": 23,
          "Recheck Cond": "(((status)::text = ANY ('{OPEN,PARTIALLY_FILLED}'::text[])) AND ((provenance)::text = ANY ('{REAL,DEMO}'::text[])) AND (product_id = products.id) AND (delivery_point_id = delivery_points.id) AND ((side)::text = 'BID'::text))",
          "Rows Removed by Index Recheck": 0,
          "Filter": "((off_spec IS FALSE) AND ((expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone) OR ((expires_at IS NULL) AND ((provenance)::text = ANY ('{REAL,UNKNOWN}'::text[])))) AND (((availability_window)::text = 'SPOT'::text) OR ((availability_window)::text ~ '^[0-9]{4}-(0[1-9]|1[0-2])$'::text) OR ((availability_window)::text ~ '^[0-9]{4}-Q[1-4]$'::text) OR ((availability_window)::text ~ '^[0-9]{4}-CAL$'::text)) AND ((product_id <> '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid) OR ((delivery_point_id = '1379d36c-1ca9-55b7-9c0d-5235a0ba1f36'::uuid) AND ((lng_terms ->> 'side'::text) = (side)::text) AND (off_spec IS FALSE))) AND ((product_id <> 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) OR ((delivery_point_id = '73835e92-820e-584b-8280-bb61c63aa28e'::uuid) AND ((fame_terms ->> 'side'::text) = (side)::text) AND (off_spec IS FALSE) AND (((side)::text <> 'ASK'::text) OR (((fame_terms ->> 'certificate_valid_until'::text) >= '2026-10-15'::text) AND (msds_available IS TRUE) AND (certification_declared IS TRUE) AND (length(TRIM(BOTH FROM COALESCE(certification_scheme, ''::character varying))) > 0))))) AND CASE WHEN ((creation_method)::text = 'MARKET_SUPPORT'::text) THEN EXISTS(SubPlan 5) ELSE ((owner_user_id IS NULL) OR EXISTS(SubPlan 9)) END)",
          "Rows Removed by Filter": 1,
          "Exact Heap Blocks": 268,
          "Lossy Heap Blocks": 0,
          "Shared Hit Blocks": 30490,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0,
          "Plans": [
            {
              "Node Type": "Bitmap Index Scan",
              "Parent Relationship": "Outer",
              "Parallel Aware": false,
              "Async Capable": false,
              "Index Name": "ix_orderbook_orders_public_aggregate",
              "Startup Cost": 0.0,
              "Total Cost": 25.96,
              "Plan Rows": 502,
              "Plan Width": 0,
              "Actual Startup Time": 0.033,
              "Actual Total Time": 0.033,
              "Actual Rows": 438,
              "Actual Loops": 23,
              "Index Cond": "(((status)::text = ANY ('{OPEN,PARTIALLY_FILLED}'::text[])) AND ((provenance)::text = ANY ('{REAL,DEMO}'::text[])) AND (product_id = products.id) AND (delivery_point_id = delivery_points.id) AND ((side)::text = 'BID'::text))",
              "Shared Hit Blocks": 106,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0
            },
            {
              "Node Type": "Result",
              "Parent Relationship": "SubPlan",
              "Subplan Name": "SubPlan 5",
              "Parallel Aware": false,
              "Async Capable": false,
              "Startup Cost": 14.05,
              "Total Cost": 15.45,
              "Plan Rows": 1,
              "Plan Width": 0,
              "Actual Startup Time": 0.017,
              "Actual Total Time": 0.017,
              "Actual Rows": 0,
              "Actual Loops": 7,
              "One-Time Filter": "(((orderbook_orders.creation_method)::text = 'MARKET_SUPPORT'::text) AND (InitPlan 3).col1 AND (InitPlan 4).col1 AND (orderbook_orders.owner_user_id = orderbook_orders.created_by_actor_user_id))",
              "Shared Hit Blocks": 22,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0,
              "Plans": [
                {
                  "Node Type": "Index Scan",
                  "Parent Relationship": "InitPlan",
                  "Subplan Name": "InitPlan 3",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Scan Direction": "Forward",
                  "Index Name": "ix_users_organization_id_id",
                  "Relation Name": "users",
                  "Alias": "users_1",
                  "Startup Cost": 0.14,
                  "Total Cost": 12.82,
                  "Plan Rows": 1,
                  "Plan Width": 0,
                  "Actual Startup Time": 0.011,
                  "Actual Total Time": 0.011,
                  "Actual Rows": 1,
                  "Actual Loops": 7,
                  "Index Cond": "(organization_id = orderbook_orders.organization_id)",
                  "Rows Removed by Index Recheck": 0,
                  "Filter": "((email_verified IS TRUE) AND (must_change_password IS FALSE) AND ((role)::text = ANY ('{BUYER,SUPPLIER}'::text[])) AND ((kyc_status)::text <> 'REJECTED'::text) AND ((status)::text = 'APPROVED'::text) AND ((((kyc_status)::text = 'APPROVED'::text) AND (kyc_organization_id = organization_id)) OR (ANY ((id = (hashed SubPlan 2).col1) AND (organization_id = (hashed SubPlan 2).col2)))))",
                  "Rows Removed by Filter": 0,
                  "Shared Hit Blocks": 15,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0,
                  "Plans": [
                    {
                      "Node Type": "Seq Scan",
                      "Parent Relationship": "SubPlan",
                      "Subplan Name": "SubPlan 2",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Relation Name": "kyc_execution_grace",
                      "Alias": "kyc_execution_grace",
                      "Startup Cost": 0.0,
                      "Total Cost": 1.1,
                      "Plan Rows": 6,
                      "Plan Width": 32,
                      "Actual Startup Time": 0.003,
                      "Actual Total Time": 0.005,
                      "Actual Rows": 6,
                      "Actual Loops": 1,
                      "Filter": "(expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone)",
                      "Rows Removed by Filter": 2,
                      "Shared Hit Blocks": 1,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0
                    }
                  ]
                },
                {
                  "Node Type": "Seq Scan",
                  "Parent Relationship": "InitPlan",
                  "Subplan Name": "InitPlan 4",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Relation Name": "organizations",
                  "Alias": "organizations",
                  "Startup Cost": 0.0,
                  "Total Cost": 1.23,
                  "Plan Rows": 1,
                  "Plan Width": 0,
                  "Actual Startup Time": 0.004,
                  "Actual Total Time": 0.004,
                  "Actual Rows": 1,
                  "Actual Loops": 4,
                  "Filter": "((id = orderbook_orders.organization_id) AND ((verification_status)::text = 'APPROVED'::text) AND ((provenance)::text = 'REAL'::text))",
                  "Rows Removed by Filter": 4,
                  "Shared Hit Blocks": 4,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0
                },
                {
                  "Node Type": "Seq Scan",
                  "Parent Relationship": "Outer",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Relation Name": "users",
                  "Alias": "users",
                  "Startup Cost": 14.05,
                  "Total Cost": 15.45,
                  "Plan Rows": 1,
                  "Plan Width": 0,
                  "Actual Startup Time": 0.006,
                  "Actual Total Time": 0.006,
                  "Actual Rows": 1,
                  "Actual Loops": 3,
                  "Filter": "((id = orderbook_orders.created_by_actor_user_id) AND ((role)::text = 'ADMIN'::text) AND ((status)::text = 'APPROVED'::text))",
                  "Rows Removed by Filter": 13,
                  "Shared Hit Blocks": 3,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0
                }
              ]
            },
            {
              "Node Type": "Result",
              "Parent Relationship": "SubPlan",
              "Subplan Name": "SubPlan 9",
              "Parallel Aware": false,
              "Async Capable": false,
              "Startup Cost": 1.33,
              "Total Cost": 10.5,
              "Plan Rows": 1,
              "Plan Width": 0,
              "Actual Startup Time": 0.006,
              "Actual Total Time": 0.006,
              "Actual Rows": 1,
              "Actual Loops": 10032,
              "One-Time Filter": "(InitPlan 8).col1",
              "Shared Hit Blocks": 30094,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0,
              "Plans": [
                {
                  "Node Type": "Seq Scan",
                  "Parent Relationship": "InitPlan",
                  "Subplan Name": "InitPlan 8",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Relation Name": "organizations",
                  "Alias": "organizations_1",
                  "Startup Cost": 0.0,
                  "Total Cost": 1.2,
                  "Plan Rows": 1,
                  "Plan Width": 0,
                  "Actual Startup Time": 0.002,
                  "Actual Total Time": 0.002,
                  "Actual Rows": 1,
                  "Actual Loops": 10032,
                  "Filter": "((id = orderbook_orders.organization_id) AND ((verification_status)::text = 'APPROVED'::text))",
                  "Rows Removed by Filter": 3,
                  "Shared Hit Blocks": 10032,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0
                },
                {
                  "Node Type": "Index Scan",
                  "Parent Relationship": "Outer",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Scan Direction": "Forward",
                  "Index Name": "ix_users_organization_id_id",
                  "Relation Name": "users",
                  "Alias": "users_2",
                  "Startup Cost": 1.33,
                  "Total Cost": 10.5,
                  "Plan Rows": 1,
                  "Plan Width": 0,
                  "Actual Startup Time": 0.003,
                  "Actual Total Time": 0.003,
                  "Actual Rows": 1,
                  "Actual Loops": 10031,
                  "Index Cond": "((organization_id = orderbook_orders.organization_id) AND (id = orderbook_orders.owner_user_id))",
                  "Rows Removed by Index Recheck": 0,
                  "Filter": "((email_verified IS TRUE) AND (must_change_password IS FALSE) AND ((role)::text = ANY ('{BUYER,SUPPLIER}'::text[])) AND ((kyc_status)::text <> 'REJECTED'::text) AND ((status)::text = 'APPROVED'::text) AND ((((kyc_status)::text = 'APPROVED'::text) AND (kyc_organization_id = organization_id)) OR (ANY ((id = (hashed SubPlan 7).col1) AND (organization_id = (hashed SubPlan 7).col2)))))",
                  "Rows Removed by Filter": 0,
                  "Shared Hit Blocks": 20062,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0,
                  "Plans": [
                    {
                      "Node Type": "Seq Scan",
                      "Parent Relationship": "SubPlan",
                      "Subplan Name": "SubPlan 7",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Relation Name": "kyc_execution_grace",
                      "Alias": "kyc_execution_grace_1",
                      "Startup Cost": 0.0,
                      "Total Cost": 1.1,
                      "Plan Rows": 6,
                      "Plan Width": 32,
                      "Actual Startup Time": 0.005,
                      "Actual Total Time": 0.007,
                      "Actual Rows": 6,
                      "Actual Loops": 1,
                      "Filter": "(expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone)",
                      "Rows Removed by Filter": 2,
                      "Shared Hit Blocks": 1,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0
                    }
                  ]
                }
              ]
            }
          ]
        }
      ]
    }
  ]
}
```

### python /api/orderbook/bids rows=20

```sql
SELECT products.id, products.name, products.fuel_type, products.fuel_grade, products.unit, products.min_lot_size, products.spec_description, products.is_active, products.created_at, delivery_points.id AS id_1, delivery_points.name AS name_1, delivery_points.region, delivery_points.timezone, delivery_points.is_active AS is_active_1, delivery_points.created_at AS created_at_1, orderbook_orders.id AS id_2, orderbook_orders.acceptance_ordinal, orderbook_orders.organization_id, orderbook_orders.owner_user_id, orderbook_orders.created_by_actor_user_id, orderbook_orders.creation_method, orderbook_orders.support_authorization_id, orderbook_orders.version, orderbook_orders.inventory_item_id, orderbook_orders.provenance, orderbook_orders.side, orderbook_orders.product_id, orderbook_orders.delivery_point_id, orderbook_orders.port_id, orderbook_orders.vessel_id, orderbook_orders.quantity_mt, orderbook_orders.remaining_quantity_mt, orderbook_orders.price_per_mt_usd, orderbook_orders.availability_window, orderbook_orders.delivery_window_start, orderbook_orders.delivery_window_end, orderbook_orders.certifications, orderbook_orders.certification_declared, orderbook_orders.certification_scheme, orderbook_orders.fame_terms, orderbook_orders.lng_terms, orderbook_orders.specification_standard, orderbook_orders.msds_available, orderbook_orders.is_verdaxis_verified, orderbook_orders.carbon_intensity_gco2_mj, orderbook_orders.carbon_intensity_method, orderbook_orders.energy_density_mj_kg, orderbook_orders.feedstock, orderbook_orders.origin, orderbook_orders.off_spec, orderbook_orders.off_spec_notes, orderbook_orders.status, orderbook_orders.expires_at, orderbook_orders.idempotency_key, orderbook_orders.idempotency_operation, orderbook_orders.idempotency_request_hash, orderbook_orders.created_at AS created_at_2, orderbook_orders.updated_at, organizations_1.id AS id_3, organizations_1.name AS name_2, organizations_1.domain, organizations_1.type, organizations_1.supplier_tier, organizations_1.tax_id, organizations_1.country_code, organizations_1.verification_status, organizations_1.provenance AS provenance_1, organizations_1.created_at AS created_at_3 
FROM orderbook_orders JOIN products ON orderbook_orders.product_id = products.id JOIN delivery_points ON orderbook_orders.delivery_point_id = delivery_points.id LEFT OUTER JOIN organizations AS organizations_1 ON organizations_1.id = orderbook_orders.organization_id 
WHERE orderbook_orders.status IN ($133, $134) AND orderbook_orders.side = $1 AND products.is_active IS true AND (products.id = $2::UUID AND products.name = $3::VARCHAR AND products.fuel_type = $4::VARCHAR AND products.fuel_grade = $5::VARCHAR OR products.id = $6::UUID AND products.name = $7::VARCHAR AND products.fuel_type = $8::VARCHAR AND products.fuel_grade = $9::VARCHAR OR products.id = $10::UUID AND products.name = $11::VARCHAR AND products.fuel_type = $12::VARCHAR AND products.fuel_grade = $13::VARCHAR OR products.id = $14::UUID AND products.name = $15::VARCHAR AND products.fuel_type = $16::VARCHAR AND products.fuel_grade = $17::VARCHAR OR products.id = $18::UUID AND products.name = $19::VARCHAR AND products.fuel_type = $20::VARCHAR AND products.fuel_grade = $21::VARCHAR OR products.id = $22::UUID AND products.name = $23::VARCHAR AND products.fuel_type = $24::VARCHAR AND products.fuel_grade = $25::VARCHAR) AND CASE WHEN (products.id = $26::UUID AND products.name = $27::VARCHAR AND products.fuel_type = $28::VARCHAR AND products.fuel_grade = $29::VARCHAR) THEN $30::VARCHAR WHEN (products.id = $31::UUID AND products.name = $32::VARCHAR AND products.fuel_type = $33::VARCHAR AND products.fuel_grade = $34::VARCHAR) THEN $35::VARCHAR WHEN (products.id = $36::UUID AND products.name = $37::VARCHAR AND products.fuel_type = $38::VARCHAR AND products.fuel_grade = $39::VARCHAR) THEN $40::VARCHAR WHEN (products.id = $41::UUID AND products.name = $42::VARCHAR AND products.fuel_type = $43::VARCHAR AND products.fuel_grade = $44::VARCHAR) THEN $45::VARCHAR WHEN (products.id = $46::UUID AND products.name = $47::VARCHAR AND products.fuel_type = $48::VARCHAR AND products.fuel_grade = $49::VARCHAR) THEN $50::VARCHAR WHEN (products.id = $51::UUID AND products.name = $52::VARCHAR AND products.fuel_type = $53::VARCHAR AND products.fuel_grade = $54::VARCHAR) THEN $55::VARCHAR END IN ($135::VARCHAR, $136::VARCHAR, $137::VARCHAR, $138::VARCHAR, $139::VARCHAR, $140::VARCHAR) AND delivery_points.is_active IS true AND (delivery_points.id = $56::UUID AND delivery_points.name = $57::VARCHAR AND delivery_points.region = $58::VARCHAR OR delivery_points.id = $59::UUID AND delivery_points.name = $60::VARCHAR AND delivery_points.region = $61::VARCHAR OR delivery_points.id = $62::UUID AND delivery_points.name = $63::VARCHAR AND delivery_points.region = $64::VARCHAR OR delivery_points.id = $65::UUID AND delivery_points.name = $66::VARCHAR AND delivery_points.region = $67::VARCHAR OR delivery_points.id = $68::UUID AND delivery_points.name = $69::VARCHAR AND delivery_points.region = $70::VARCHAR OR delivery_points.id = $71::UUID AND delivery_points.name = $72::VARCHAR AND delivery_points.region = $73::VARCHAR OR delivery_points.id = $74::UUID AND delivery_points.name = $75::VARCHAR AND delivery_points.region = $76::VARCHAR OR delivery_points.id = $77::UUID AND delivery_points.name = $78::VARCHAR AND delivery_points.region = $79::VARCHAR) AND (products.id = $80::UUID AND delivery_points.id IN ($141::UUID) OR products.id = $81::UUID AND delivery_points.id IN ($142::UUID) OR products.id = $82::UUID OR products.id = $83::UUID OR products.id = $84::UUID OR products.id = $85::UUID) AND (orderbook_orders.availability_window = $86::VARCHAR OR orderbook_orders.availability_window ~ $87::VARCHAR OR orderbook_orders.availability_window ~ $88::VARCHAR OR orderbook_orders.availability_window ~ $89::VARCHAR) AND (orderbook_orders.expires_at > $90::TIMESTAMP WITH TIME ZONE OR orderbook_orders.expires_at IS NULL AND orderbook_orders.provenance IN ($143, $144)) AND CASE WHEN (orderbook_orders.creation_method = $91) THEN EXISTS (SELECT users.id 
FROM users 
WHERE orderbook_orders.creation_method = $92 AND users.id = orderbook_orders.owner_user_id AND users.id = orderbook_orders.created_by_actor_user_id AND users.role = $93 AND users.status = $94 AND (EXISTS (SELECT users_1.id 
FROM users AS users_1 
WHERE users_1.organization_id = orderbook_orders.organization_id AND users_1.role IN ($145, $146) AND users_1.status = $95 AND users_1.email_verified IS true AND users_1.must_change_password IS false AND users_1.kyc_status != $96::VARCHAR AND (users_1.kyc_status = $97::VARCHAR AND users_1.kyc_organization_id = users_1.organization_id OR (EXISTS (SELECT kyc_execution_grace.id 
FROM kyc_execution_grace 
WHERE kyc_execution_grace.user_id = users_1.id AND kyc_execution_grace.organization_id = users_1.organization_id AND kyc_execution_grace.expires_at > $98::TIMESTAMP WITH TIME ZONE))))) AND (EXISTS (SELECT organizations.id 
FROM organizations 
WHERE organizations.id = orderbook_orders.organization_id AND organizations.verification_status = $99::VARCHAR AND organizations.provenance = $100))) ELSE orderbook_orders.owner_user_id IS NULL OR (EXISTS (SELECT users.id 
FROM users 
WHERE users.id = orderbook_orders.owner_user_id AND users.organization_id = orderbook_orders.organization_id AND users.role IN ($147, $148) AND users.status = $101 AND users.email_verified IS true AND users.must_change_password IS false AND users.kyc_status != $102::VARCHAR AND (users.kyc_status = $103::VARCHAR AND users.kyc_organization_id = users.organization_id OR (EXISTS (SELECT kyc_execution_grace.id 
FROM kyc_execution_grace 
WHERE kyc_execution_grace.user_id = users.id AND kyc_execution_grace.organization_id = users.organization_id AND kyc_execution_grace.expires_at > $104::TIMESTAMP WITH TIME ZONE))) AND (EXISTS (SELECT organizations.id 
FROM organizations 
WHERE organizations.id = orderbook_orders.organization_id AND organizations.verification_status = $105::VARCHAR)))) END AND (orderbook_orders.product_id != $106::UUID OR orderbook_orders.delivery_point_id IN ($149::UUID) AND CAST((orderbook_orders.fame_terms ->> $107::TEXT) AS VARCHAR) = CAST(orderbook_orders.side AS VARCHAR) AND orderbook_orders.off_spec IS false AND (orderbook_orders.side != $108 OR CAST((orderbook_orders.fame_terms ->> $109::TEXT) AS VARCHAR) >= $110::VARCHAR AND orderbook_orders.msds_available IS true AND orderbook_orders.certification_declared IS true AND length(trim(coalesce(orderbook_orders.certification_scheme, $111::VARCHAR))) > $112::INTEGER)) AND (orderbook_orders.product_id != $113::UUID OR orderbook_orders.delivery_point_id IN ($150::UUID) AND CAST((orderbook_orders.lng_terms ->> $114::TEXT) AS VARCHAR) = CAST(orderbook_orders.side AS VARCHAR) AND orderbook_orders.off_spec IS false) AND orderbook_orders.provenance IN ($151, $152) AND products.fuel_type IN ($153::VARCHAR, $154::VARCHAR, $155::VARCHAR, $156::VARCHAR) AND orderbook_orders.off_spec IS false AND (orderbook_orders.side != $115 OR length(trim(coalesce(orderbook_orders.certification_scheme, $116::VARCHAR))) > $117::INTEGER) AND (orderbook_orders.side != $118 OR orderbook_orders.certification_declared IS true) AND (orderbook_orders.side != $119 OR length(trim(coalesce(orderbook_orders.specification_standard, $120::VARCHAR))) > $121::INTEGER) AND (orderbook_orders.side != $122 OR orderbook_orders.msds_available IS true) AND (orderbook_orders.side != $123 OR orderbook_orders.product_id IN ($157::UUID, $158::UUID) OR orderbook_orders.carbon_intensity_gco2_mj IS NOT NULL) AND (orderbook_orders.side != $124 OR length(trim(coalesce(orderbook_orders.feedstock, $125::VARCHAR))) > $126::INTEGER) AND (orderbook_orders.side != $127 OR orderbook_orders.product_id = $128::UUID OR length(trim(coalesce(orderbook_orders.origin, $129::VARCHAR))) > $130::INTEGER) ORDER BY orderbook_orders.created_at DESC, orderbook_orders.id DESC 
 LIMIT $131::INTEGER OFFSET $132::INTEGER
```

```json
{
  "Node Type": "Limit",
  "Parallel Aware": false,
  "Async Capable": false,
  "Startup Cost": 13412.71,
  "Total Cost": 13412.76,
  "Plan Rows": 20,
  "Plan Width": 3350,
  "Actual Startup Time": 161.539,
  "Actual Total Time": 161.561,
  "Actual Rows": 20,
  "Actual Loops": 1,
  "Shared Hit Blocks": 30497,
  "Shared Read Blocks": 0,
  "Shared Dirtied Blocks": 0,
  "Shared Written Blocks": 0,
  "Local Hit Blocks": 0,
  "Local Read Blocks": 0,
  "Local Dirtied Blocks": 0,
  "Local Written Blocks": 0,
  "Temp Read Blocks": 0,
  "Temp Written Blocks": 0,
  "Plans": [
    {
      "Node Type": "Sort",
      "Parent Relationship": "Outer",
      "Parallel Aware": false,
      "Async Capable": false,
      "Startup Cost": 13412.71,
      "Total Cost": 13412.87,
      "Plan Rows": 62,
      "Plan Width": 3350,
      "Actual Startup Time": 161.537,
      "Actual Total Time": 161.553,
      "Actual Rows": 20,
      "Actual Loops": 1,
      "Sort Key": [
        "orderbook_orders.created_at DESC",
        "orderbook_orders.id DESC"
      ],
      "Sort Method": "top-N heapsort",
      "Sort Space Used": 43,
      "Sort Space Type": "Memory",
      "Shared Hit Blocks": 30497,
      "Shared Read Blocks": 0,
      "Shared Dirtied Blocks": 0,
      "Shared Written Blocks": 0,
      "Local Hit Blocks": 0,
      "Local Read Blocks": 0,
      "Local Dirtied Blocks": 0,
      "Local Written Blocks": 0,
      "Temp Read Blocks": 0,
      "Temp Written Blocks": 0,
      "Plans": [
        {
          "Node Type": "Nested Loop",
          "Parent Relationship": "Outer",
          "Parallel Aware": false,
          "Async Capable": false,
          "Join Type": "Left",
          "Startup Cost": 26.02,
          "Total Cost": 13411.06,
          "Plan Rows": 62,
          "Plan Width": 3350,
          "Actual Startup Time": 1.296,
          "Actual Total Time": 138.981,
          "Actual Rows": 10023,
          "Actual Loops": 1,
          "Inner Unique": true,
          "Join Filter": "(organizations_1.id = orderbook_orders.organization_id)",
          "Rows Removed by Join Filter": 30087,
          "Shared Hit Blocks": 30497,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0,
          "Plans": [
            {
              "Node Type": "Nested Loop",
              "Parent Relationship": "Outer",
              "Parallel Aware": false,
              "Async Capable": false,
              "Join Type": "Inner",
              "Startup Cost": 26.02,
              "Total Cost": 13398.64,
              "Plan Rows": 62,
              "Plan Width": 3087,
              "Actual Startup Time": 1.254,
              "Actual Total Time": 123.282,
              "Actual Rows": 10023,
              "Actual Loops": 1,
              "Inner Unique": false,
              "Shared Hit Blocks": 30496,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0,
              "Plans": [
                {
                  "Node Type": "Nested Loop",
                  "Parent Relationship": "Outer",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Join Type": "Inner",
                  "Startup Cost": 0.0,
                  "Total Cost": 3.76,
                  "Plan Rows": 1,
                  "Plan Width": 204,
                  "Actual Startup Time": 0.068,
                  "Actual Total Time": 0.197,
                  "Actual Rows": 23,
                  "Actual Loops": 1,
                  "Inner Unique": false,
                  "Join Filter": "(((products.id = '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid) AND (delivery_points.id = '1379d36c-1ca9-55b7-9c0d-5235a0ba1f36'::uuid)) OR ((products.id = 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) AND (delivery_points.id = '73835e92-820e-584b-8280-bb61c63aa28e'::uuid)) OR (products.id = '9510c713-6e39-5080-add3-0a7c29657b79'::uuid) OR (products.id = 'f9b20492-b445-59cd-b292-a386d913f488'::uuid) OR (products.id = 'c4a688be-f7c2-5edc-8f93-6b34e387609c'::uuid) OR (products.id = 'd186bffb-766d-5944-8825-989abbdcfc46'::uuid))",
                  "Rows Removed by Join Filter": 12,
                  "Shared Hit Blocks": 6,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0,
                  "Plans": [
                    {
                      "Node Type": "Seq Scan",
                      "Parent Relationship": "Outer",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Relation Name": "products",
                      "Alias": "products",
                      "Startup Cost": 0.0,
                      "Total Cost": 2.1,
                      "Plan Rows": 1,
                      "Plan Width": 149,
                      "Actual Startup Time": 0.046,
                      "Actual Total Time": 0.065,
                      "Actual Rows": 5,
                      "Actual Loops": 1,
                      "Filter": "((is_active IS TRUE) AND ((fuel_type)::text = ANY ('{Methanol,Ethanol,FAME,LNG}'::text[])) AND ((id = '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid) OR (id = 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) OR (id = '9510c713-6e39-5080-add3-0a7c29657b79'::uuid) OR (id = 'f9b20492-b445-59cd-b292-a386d913f488'::uuid) OR (id = 'c4a688be-f7c2-5edc-8f93-6b34e387609c'::uuid) OR (id = 'd186bffb-766d-5944-8825-989abbdcfc46'::uuid)) AND (((id = '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid) AND ((name)::text = 'Bio-LNG'::text) AND ((fuel_type)::text = 'LNG'::text) AND ((fuel_grade)::text = 'Bio'::text)) OR ((id = 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) AND ((name)::text = 'UCOME B100'::text) AND ((fuel_type)::text = 'FAME'::text) AND ((fuel_grade)::text = 'UCOME'::text)) OR ((id = '9510c713-6e39-5080-add3-0a7c29657b79'::uuid) AND ((name)::text = 'Bio Methanol'::text) AND ((fuel_type)::text = 'Methanol'::text) AND ((fuel_grade)::text = 'Bio'::text)) OR ((id = 'f9b20492-b445-59cd-b292-a386d913f488'::uuid) AND ((name)::text = 'e-Methanol'::text) AND ((fuel_type)::text = 'Methanol'::text) AND ((fuel_grade)::text = 'E'::text)) OR ((id = 'c4a688be-f7c2-5edc-8f93-6b34e387609c'::uuid) AND ((name)::text = 'Bio Ethanol'::text) AND ((fuel_type)::text = 'Ethanol'::text) AND ((fuel_grade)::text = 'Bio'::text)) OR ((id = 'd186bffb-766d-5944-8825-989abbdcfc46'::uuid) AND ((name)::text = 'Synthetic Ethanol'::text) AND ((fuel_type)::text = 'Ethanol'::text) AND ((fuel_grade)::text = 'Synthetic'::text))) AND ((CASE WHEN ((id = '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid) AND ((name)::text = 'Bio-LNG'::text) AND ((fuel_type)::text = 'LNG'::text) AND ((fuel_grade)::text = 'Bio'::text)) THEN 'BIO_LNG'::character varying WHEN ((id = 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) AND ((name)::text = 'UCOME B100'::text) AND ((fuel_type)::text = 'FAME'::text) AND ((fuel_grade)::text = 'UCOME'::text)) THEN 'UCOME_B100'::character varying WHEN ((id = '9510c713-6e39-5080-add3-0a7c29657b79'::uuid) AND ((name)::text = 'Bio Methanol'::text) AND ((fuel_type)::text = 'Methanol'::text) AND ((fuel_grade)::text = 'Bio'::text)) THEN 'BIO_METHANOL'::character varying WHEN ((id = 'f9b20492-b445-59cd-b292-a386d913f488'::uuid) AND ((name)::text = 'e-Methanol'::text) AND ((fuel_type)::text = 'Methanol'::text) AND ((fuel_grade)::text = 'E'::text)) THEN 'E_METHANOL'::character varying WHEN ((id = 'c4a688be-f7c2-5edc-8f93-6b34e387609c'::uuid) AND ((name)::text = 'Bio Ethanol'::text) AND ((fuel_type)::text = 'Ethanol'::text) AND ((fuel_grade)::text = 'Bio'::text)) THEN 'BIO_ETHANOL'::character varying WHEN ((id = 'd186bffb-766d-5944-8825-989abbdcfc46'::uuid) AND ((name)::text = 'Synthetic Ethanol'::text) AND ((fuel_type)::text = 'Ethanol'::text) AND ((fuel_grade)::text = 'Synthetic'::text)) THEN 'SYNTHETIC_ETHANOL'::character varying ELSE NULL::character varying END)::text = ANY ('{BIO_LNG,UCOME_B100,BIO_METHANOL,E_METHANOL,BIO_ETHANOL,SYNTHETIC_ETHANOL}'::text[])))",
                      "Rows Removed by Filter": 2,
                      "Shared Hit Blocks": 1,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0
                    },
                    {
                      "Node Type": "Seq Scan",
                      "Parent Relationship": "Inner",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Relation Name": "delivery_points",
                      "Alias": "delivery_points",
                      "Startup Cost": 0.0,
                      "Total Cost": 1.63,
                      "Plan Rows": 1,
                      "Plan Width": 55,
                      "Actual Startup Time": 0.005,
                      "Actual Total Time": 0.014,
                      "Actual Rows": 7,
                      "Actual Loops": 5,
                      "Filter": "((is_active IS TRUE) AND (((id = '78281fb3-e726-5396-802e-34b01fda21e8'::uuid) AND ((name)::text = 'Dalian'::text) AND ((region)::text = 'Asia'::text)) OR ((id = '262d36ae-6f35-5785-b9e8-9e221b0f1b78'::uuid) AND ((name)::text = 'Busan'::text) AND ((region)::text = 'Asia'::text)) OR ((id = '633c0593-f9b9-52ad-abe5-a0285f6888d3'::uuid) AND ((name)::text = 'Shanghai'::text) AND ((region)::text = 'Asia'::text)) OR ((id = '73835e92-820e-584b-8280-bb61c63aa28e'::uuid) AND ((name)::text = 'Singapore'::text) AND ((region)::text = 'Asia'::text)) OR ((id = '1379d36c-1ca9-55b7-9c0d-5235a0ba1f36'::uuid) AND ((name)::text = 'Rotterdam'::text) AND ((region)::text = 'Europe'::text)) OR ((id = 'a083db06-b050-56c2-a274-3897eac2fdae'::uuid) AND ((name)::text = 'Houston'::text) AND ((region)::text = 'Americas'::text)) OR ((id = '1be2a5cb-fc34-5e88-8b15-7f7d0fd6e534'::uuid) AND ((name)::text = 'Los Angeles'::text) AND ((region)::text = 'Americas'::text)) OR ((id = 'a4762444-4e0b-5bc1-b647-a185047adae5'::uuid) AND ((name)::text = 'Santos'::text) AND ((region)::text = 'Americas'::text))))",
                      "Rows Removed by Filter": 2,
                      "Shared Hit Blocks": 5,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0
                    }
                  ]
                },
                {
                  "Node Type": "Bitmap Heap Scan",
                  "Parent Relationship": "Inner",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Relation Name": "orderbook_orders",
                  "Alias": "orderbook_orders",
                  "Startup Cost": 26.02,
                  "Total Cost": 13392.42,
                  "Plan Rows": 246,
                  "Plan Width": 2883,
                  "Actual Startup Time": 0.059,
                  "Actual Total Time": 5.136,
                  "Actual Rows": 436,
                  "Actual Loops": 23,
                  "Recheck Cond": "(((status)::text = ANY ('{OPEN,PARTIALLY_FILLED}'::text[])) AND ((provenance)::text = ANY ('{REAL,DEMO}'::text[])) AND (product_id = products.id) AND (delivery_point_id = delivery_points.id) AND ((side)::text = 'BID'::text))",
                  "Rows Removed by Index Recheck": 0,
                  "Filter": "((off_spec IS FALSE) AND ((expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone) OR ((expires_at IS NULL) AND ((provenance)::text = ANY ('{REAL,UNKNOWN}'::text[])))) AND (((availability_window)::text = 'SPOT'::text) OR ((availability_window)::text ~ '^[0-9]{4}-(0[1-9]|1[0-2])$'::text) OR ((availability_window)::text ~ '^[0-9]{4}-Q[1-4]$'::text) OR ((availability_window)::text ~ '^[0-9]{4}-CAL$'::text)) AND ((product_id <> '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid) OR ((delivery_point_id = '1379d36c-1ca9-55b7-9c0d-5235a0ba1f36'::uuid) AND ((lng_terms ->> 'side'::text) = (side)::text) AND (off_spec IS FALSE))) AND ((product_id <> 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) OR ((delivery_point_id = '73835e92-820e-584b-8280-bb61c63aa28e'::uuid) AND ((fame_terms ->> 'side'::text) = (side)::text) AND (off_spec IS FALSE) AND (((side)::text <> 'ASK'::text) OR (((fame_terms ->> 'certificate_valid_until'::text) >= '2026-10-15'::text) AND (msds_available IS TRUE) AND (certification_declared IS TRUE) AND (length(TRIM(BOTH FROM COALESCE(certification_scheme, ''::character varying))) > 0))))) AND CASE WHEN ((creation_method)::text = 'MARKET_SUPPORT'::text) THEN EXISTS(SubPlan 5) ELSE ((owner_user_id IS NULL) OR EXISTS(SubPlan 9)) END)",
                  "Rows Removed by Filter": 1,
                  "Exact Heap Blocks": 268,
                  "Lossy Heap Blocks": 0,
                  "Shared Hit Blocks": 30490,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0,
                  "Plans": [
                    {
                      "Node Type": "Bitmap Index Scan",
                      "Parent Relationship": "Outer",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Index Name": "ix_orderbook_orders_public_aggregate",
                      "Startup Cost": 0.0,
                      "Total Cost": 25.96,
                      "Plan Rows": 502,
                      "Plan Width": 0,
                      "Actual Startup Time": 0.051,
                      "Actual Total Time": 0.051,
                      "Actual Rows": 438,
                      "Actual Loops": 23,
                      "Index Cond": "(((status)::text = ANY ('{OPEN,PARTIALLY_FILLED}'::text[])) AND ((provenance)::text = ANY ('{REAL,DEMO}'::text[])) AND (product_id = products.id) AND (delivery_point_id = delivery_points.id) AND ((side)::text = 'BID'::text))",
                      "Shared Hit Blocks": 106,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0
                    },
                    {
                      "Node Type": "Result",
                      "Parent Relationship": "SubPlan",
                      "Subplan Name": "SubPlan 5",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Startup Cost": 14.05,
                      "Total Cost": 15.45,
                      "Plan Rows": 1,
                      "Plan Width": 0,
                      "Actual Startup Time": 0.016,
                      "Actual Total Time": 0.016,
                      "Actual Rows": 0,
                      "Actual Loops": 7,
                      "One-Time Filter": "(((orderbook_orders.creation_method)::text = 'MARKET_SUPPORT'::text) AND (InitPlan 3).col1 AND (InitPlan 4).col1 AND (orderbook_orders.owner_user_id = orderbook_orders.created_by_actor_user_id))",
                      "Shared Hit Blocks": 22,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0,
                      "Plans": [
                        {
                          "Node Type": "Index Scan",
                          "Parent Relationship": "InitPlan",
                          "Subplan Name": "InitPlan 3",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Scan Direction": "Forward",
                          "Index Name": "ix_users_organization_id_id",
                          "Relation Name": "users",
                          "Alias": "users_1",
                          "Startup Cost": 0.14,
                          "Total Cost": 12.82,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.01,
                          "Actual Total Time": 0.01,
                          "Actual Rows": 1,
                          "Actual Loops": 7,
                          "Index Cond": "(organization_id = orderbook_orders.organization_id)",
                          "Rows Removed by Index Recheck": 0,
                          "Filter": "((email_verified IS TRUE) AND (must_change_password IS FALSE) AND ((role)::text = ANY ('{BUYER,SUPPLIER}'::text[])) AND ((kyc_status)::text <> 'REJECTED'::text) AND ((status)::text = 'APPROVED'::text) AND ((((kyc_status)::text = 'APPROVED'::text) AND (kyc_organization_id = organization_id)) OR (ANY ((id = (hashed SubPlan 2).col1) AND (organization_id = (hashed SubPlan 2).col2)))))",
                          "Rows Removed by Filter": 0,
                          "Shared Hit Blocks": 15,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0,
                          "Plans": [
                            {
                              "Node Type": "Seq Scan",
                              "Parent Relationship": "SubPlan",
                              "Subplan Name": "SubPlan 2",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Relation Name": "kyc_execution_grace",
                              "Alias": "kyc_execution_grace",
                              "Startup Cost": 0.0,
                              "Total Cost": 1.1,
                              "Plan Rows": 6,
                              "Plan Width": 32,
                              "Actual Startup Time": 0.01,
                              "Actual Total Time": 0.012,
                              "Actual Rows": 6,
                              "Actual Loops": 1,
                              "Filter": "(expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone)",
                              "Rows Removed by Filter": 2,
                              "Shared Hit Blocks": 1,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            }
                          ]
                        },
                        {
                          "Node Type": "Seq Scan",
                          "Parent Relationship": "InitPlan",
                          "Subplan Name": "InitPlan 4",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Relation Name": "organizations",
                          "Alias": "organizations",
                          "Startup Cost": 0.0,
                          "Total Cost": 1.23,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.003,
                          "Actual Total Time": 0.003,
                          "Actual Rows": 1,
                          "Actual Loops": 4,
                          "Filter": "((id = orderbook_orders.organization_id) AND ((verification_status)::text = 'APPROVED'::text) AND ((provenance)::text = 'REAL'::text))",
                          "Rows Removed by Filter": 4,
                          "Shared Hit Blocks": 4,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0
                        },
                        {
                          "Node Type": "Seq Scan",
                          "Parent Relationship": "Outer",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Relation Name": "users",
                          "Alias": "users",
                          "Startup Cost": 14.05,
                          "Total Cost": 15.45,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.006,
                          "Actual Total Time": 0.006,
                          "Actual Rows": 1,
                          "Actual Loops": 3,
                          "Filter": "((id = orderbook_orders.created_by_actor_user_id) AND ((role)::text = 'ADMIN'::text) AND ((status)::text = 'APPROVED'::text))",
                          "Rows Removed by Filter": 13,
                          "Shared Hit Blocks": 3,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0
                        }
                      ]
                    },
                    {
                      "Node Type": "Result",
                      "Parent Relationship": "SubPlan",
                      "Subplan Name": "SubPlan 9",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Startup Cost": 1.33,
                      "Total Cost": 10.5,
                      "Plan Rows": 1,
                      "Plan Width": 0,
                      "Actual Startup Time": 0.007,
                      "Actual Total Time": 0.007,
                      "Actual Rows": 1,
                      "Actual Loops": 10032,
                      "One-Time Filter": "(InitPlan 8).col1",
                      "Shared Hit Blocks": 30094,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0,
                      "Plans": [
                        {
                          "Node Type": "Seq Scan",
                          "Parent Relationship": "InitPlan",
                          "Subplan Name": "InitPlan 8",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Relation Name": "organizations",
                          "Alias": "organizations_2",
                          "Startup Cost": 0.0,
                          "Total Cost": 1.2,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.002,
                          "Actual Total Time": 0.002,
                          "Actual Rows": 1,
                          "Actual Loops": 10032,
                          "Filter": "((id = orderbook_orders.organization_id) AND ((verification_status)::text = 'APPROVED'::text))",
                          "Rows Removed by Filter": 3,
                          "Shared Hit Blocks": 10032,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0
                        },
                        {
                          "Node Type": "Index Scan",
                          "Parent Relationship": "Outer",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Scan Direction": "Forward",
                          "Index Name": "ix_users_organization_id_id",
                          "Relation Name": "users",
                          "Alias": "users_2",
                          "Startup Cost": 1.33,
                          "Total Cost": 10.5,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.003,
                          "Actual Total Time": 0.003,
                          "Actual Rows": 1,
                          "Actual Loops": 10031,
                          "Index Cond": "((organization_id = orderbook_orders.organization_id) AND (id = orderbook_orders.owner_user_id))",
                          "Rows Removed by Index Recheck": 0,
                          "Filter": "((email_verified IS TRUE) AND (must_change_password IS FALSE) AND ((role)::text = ANY ('{BUYER,SUPPLIER}'::text[])) AND ((kyc_status)::text <> 'REJECTED'::text) AND ((status)::text = 'APPROVED'::text) AND ((((kyc_status)::text = 'APPROVED'::text) AND (kyc_organization_id = organization_id)) OR (ANY ((id = (hashed SubPlan 7).col1) AND (organization_id = (hashed SubPlan 7).col2)))))",
                          "Rows Removed by Filter": 0,
                          "Shared Hit Blocks": 20062,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0,
                          "Plans": [
                            {
                              "Node Type": "Seq Scan",
                              "Parent Relationship": "SubPlan",
                              "Subplan Name": "SubPlan 7",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Relation Name": "kyc_execution_grace",
                              "Alias": "kyc_execution_grace_1",
                              "Startup Cost": 0.0,
                              "Total Cost": 1.1,
                              "Plan Rows": 6,
                              "Plan Width": 32,
                              "Actual Startup Time": 0.014,
                              "Actual Total Time": 0.016,
                              "Actual Rows": 6,
                              "Actual Loops": 1,
                              "Filter": "(expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone)",
                              "Rows Removed by Filter": 2,
                              "Shared Hit Blocks": 1,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            }
                          ]
                        }
                      ]
                    }
                  ]
                }
              ]
            },
            {
              "Node Type": "Materialize",
              "Parent Relationship": "Inner",
              "Parallel Aware": false,
              "Async Capable": false,
              "Startup Cost": 0.0,
              "Total Cost": 1.19,
              "Plan Rows": 13,
              "Plan Width": 263,
              "Actual Startup Time": 0.0,
              "Actual Total Time": 0.0,
              "Actual Rows": 4,
              "Actual Loops": 10023,
              "Shared Hit Blocks": 1,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0,
              "Plans": [
                {
                  "Node Type": "Seq Scan",
                  "Parent Relationship": "Outer",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Relation Name": "organizations",
                  "Alias": "organizations_1",
                  "Startup Cost": 0.0,
                  "Total Cost": 1.13,
                  "Plan Rows": 13,
                  "Plan Width": 263,
                  "Actual Startup Time": 0.021,
                  "Actual Total Time": 0.025,
                  "Actual Rows": 13,
                  "Actual Loops": 1,
                  "Shared Hit Blocks": 1,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0
                }
              ]
            }
          ]
        }
      ]
    }
  ]
}
```

### python /api/orderbook/bids rows=10000

```sql
SELECT orderbook_orders.product_id, products.name AS product_name, products.fuel_type AS product_fuel_type, products.fuel_grade AS product_fuel_grade, products.is_active AS product_is_active, orderbook_orders.side, CASE WHEN (products.id = $1::UUID AND products.name = $2::VARCHAR AND products.fuel_type = $3::VARCHAR AND products.fuel_grade = $4::VARCHAR) THEN $5::VARCHAR WHEN (products.id = $6::UUID AND products.name = $7::VARCHAR AND products.fuel_type = $8::VARCHAR AND products.fuel_grade = $9::VARCHAR) THEN $10::VARCHAR WHEN (products.id = $11::UUID AND products.name = $12::VARCHAR AND products.fuel_type = $13::VARCHAR AND products.fuel_grade = $14::VARCHAR) THEN $15::VARCHAR WHEN (products.id = $16::UUID AND products.name = $17::VARCHAR AND products.fuel_type = $18::VARCHAR AND products.fuel_grade = $19::VARCHAR) THEN $20::VARCHAR WHEN (products.id = $21::UUID AND products.name = $22::VARCHAR AND products.fuel_type = $23::VARCHAR AND products.fuel_grade = $24::VARCHAR) THEN $25::VARCHAR WHEN (products.id = $26::UUID AND products.name = $27::VARCHAR AND products.fuel_type = $28::VARCHAR AND products.fuel_grade = $29::VARCHAR) THEN $30::VARCHAR END AS market_product, orderbook_orders.delivery_point_id, orderbook_orders.availability_window, orderbook_orders.status, orderbook_orders.expires_at, orderbook_orders.remaining_quantity_mt, orderbook_orders.price_per_mt_usd, orderbook_orders.off_spec, orderbook_orders.certification_declared, orderbook_orders.certification_scheme, orderbook_orders.specification_standard, orderbook_orders.msds_available, orderbook_orders.carbon_intensity_gco2_mj, orderbook_orders.carbon_intensity_method, orderbook_orders.feedstock, orderbook_orders.origin, orderbook_orders.fame_terms, orderbook_orders.lng_terms 
FROM orderbook_orders JOIN products ON orderbook_orders.product_id = products.id JOIN delivery_points ON orderbook_orders.delivery_point_id = delivery_points.id 
WHERE (orderbook_orders.side, CASE WHEN (products.id = $1::UUID AND products.name = $2::VARCHAR AND products.fuel_type = $3::VARCHAR AND products.fuel_grade = $4::VARCHAR) THEN $5::VARCHAR WHEN (products.id = $6::UUID AND products.name = $7::VARCHAR AND products.fuel_type = $8::VARCHAR AND products.fuel_grade = $9::VARCHAR) THEN $10::VARCHAR WHEN (products.id = $11::UUID AND products.name = $12::VARCHAR AND products.fuel_type = $13::VARCHAR AND products.fuel_grade = $14::VARCHAR) THEN $15::VARCHAR WHEN (products.id = $16::UUID AND products.name = $17::VARCHAR AND products.fuel_type = $18::VARCHAR AND products.fuel_grade = $19::VARCHAR) THEN $20::VARCHAR WHEN (products.id = $21::UUID AND products.name = $22::VARCHAR AND products.fuel_type = $23::VARCHAR AND products.fuel_grade = $24::VARCHAR) THEN $25::VARCHAR WHEN (products.id = $26::UUID AND products.name = $27::VARCHAR AND products.fuel_type = $28::VARCHAR AND products.fuel_grade = $29::VARCHAR) THEN $30::VARCHAR END, orderbook_orders.delivery_point_id, orderbook_orders.availability_window) IN (($105, $106, $107, $108)) AND orderbook_orders.status IN ($109, $110) AND orderbook_orders.remaining_quantity_mt > $31::INTEGER AND (orderbook_orders.expires_at > $32::TIMESTAMP WITH TIME ZONE OR orderbook_orders.expires_at IS NULL AND orderbook_orders.provenance IN ($111, $112)) AND CASE WHEN (orderbook_orders.creation_method = $33) THEN EXISTS (SELECT users.id 
FROM users 
WHERE orderbook_orders.creation_method = $34 AND users.id = orderbook_orders.owner_user_id AND users.id = orderbook_orders.created_by_actor_user_id AND users.role = $35 AND users.status = $36 AND (EXISTS (SELECT users_1.id 
FROM users AS users_1 
WHERE users_1.organization_id = orderbook_orders.organization_id AND users_1.role IN ($113, $114) AND users_1.status = $37 AND users_1.email_verified IS true AND users_1.must_change_password IS false AND users_1.kyc_status != $38::VARCHAR AND (users_1.kyc_status = $39::VARCHAR AND users_1.kyc_organization_id = users_1.organization_id OR (EXISTS (SELECT kyc_execution_grace.id 
FROM kyc_execution_grace 
WHERE kyc_execution_grace.user_id = users_1.id AND kyc_execution_grace.organization_id = users_1.organization_id AND kyc_execution_grace.expires_at > $40::TIMESTAMP WITH TIME ZONE))))) AND (EXISTS (SELECT organizations.id 
FROM organizations 
WHERE organizations.id = orderbook_orders.organization_id AND organizations.verification_status = $41::VARCHAR AND organizations.provenance = $42))) ELSE orderbook_orders.owner_user_id IS NULL OR (EXISTS (SELECT users.id 
FROM users 
WHERE users.id = orderbook_orders.owner_user_id AND users.organization_id = orderbook_orders.organization_id AND users.role IN ($115, $116) AND users.status = $43 AND users.email_verified IS true AND users.must_change_password IS false AND users.kyc_status != $44::VARCHAR AND (users.kyc_status = $45::VARCHAR AND users.kyc_organization_id = users.organization_id OR (EXISTS (SELECT kyc_execution_grace.id 
FROM kyc_execution_grace 
WHERE kyc_execution_grace.user_id = users.id AND kyc_execution_grace.organization_id = users.organization_id AND kyc_execution_grace.expires_at > $46::TIMESTAMP WITH TIME ZONE))) AND (EXISTS (SELECT organizations.id 
FROM organizations 
WHERE organizations.id = orderbook_orders.organization_id AND organizations.verification_status = $47::VARCHAR)))) END AND (orderbook_orders.product_id != $48::UUID OR orderbook_orders.delivery_point_id IN ($117::UUID) AND CAST((orderbook_orders.fame_terms ->> $49::TEXT) AS VARCHAR) = CAST(orderbook_orders.side AS VARCHAR) AND orderbook_orders.off_spec IS false AND (orderbook_orders.side != $50 OR CAST((orderbook_orders.fame_terms ->> $51::TEXT) AS VARCHAR) >= $52::VARCHAR AND orderbook_orders.msds_available IS true AND orderbook_orders.certification_declared IS true AND length(trim(coalesce(orderbook_orders.certification_scheme, $53::VARCHAR))) > $54::INTEGER)) AND (orderbook_orders.product_id != $55::UUID OR orderbook_orders.delivery_point_id IN ($118::UUID) AND CAST((orderbook_orders.lng_terms ->> $56::TEXT) AS VARCHAR) = CAST(orderbook_orders.side AS VARCHAR) AND orderbook_orders.off_spec IS false) AND orderbook_orders.provenance IN ($119, $120) AND products.is_active IS true AND (products.id = $57::UUID AND products.name = $58::VARCHAR AND products.fuel_type = $59::VARCHAR AND products.fuel_grade = $60::VARCHAR OR products.id = $61::UUID AND products.name = $62::VARCHAR AND products.fuel_type = $63::VARCHAR AND products.fuel_grade = $64::VARCHAR OR products.id = $65::UUID AND products.name = $66::VARCHAR AND products.fuel_type = $67::VARCHAR AND products.fuel_grade = $68::VARCHAR OR products.id = $69::UUID AND products.name = $70::VARCHAR AND products.fuel_type = $71::VARCHAR AND products.fuel_grade = $72::VARCHAR OR products.id = $73::UUID AND products.name = $74::VARCHAR AND products.fuel_type = $75::VARCHAR AND products.fuel_grade = $76::VARCHAR OR products.id = $77::UUID AND products.name = $78::VARCHAR AND products.fuel_type = $79::VARCHAR AND products.fuel_grade = $80::VARCHAR) AND delivery_points.is_active IS true AND (delivery_points.id = $81::UUID AND delivery_points.name = $82::VARCHAR AND delivery_points.region = $83::VARCHAR OR delivery_points.id = $84::UUID AND delivery_points.name = $85::VARCHAR AND delivery_points.region = $86::VARCHAR OR delivery_points.id = $87::UUID AND delivery_points.name = $88::VARCHAR AND delivery_points.region = $89::VARCHAR OR delivery_points.id = $90::UUID AND delivery_points.name = $91::VARCHAR AND delivery_points.region = $92::VARCHAR OR delivery_points.id = $93::UUID AND delivery_points.name = $94::VARCHAR AND delivery_points.region = $95::VARCHAR OR delivery_points.id = $96::UUID AND delivery_points.name = $97::VARCHAR AND delivery_points.region = $98::VARCHAR OR delivery_points.id = $99::UUID AND delivery_points.name = $100::VARCHAR AND delivery_points.region = $101::VARCHAR OR delivery_points.id = $102::UUID AND delivery_points.name = $103::VARCHAR AND delivery_points.region = $104::VARCHAR)
```

```json
{
  "Node Type": "Nested Loop",
  "Parallel Aware": false,
  "Async Capable": false,
  "Join Type": "Inner",
  "Startup Cost": 0.29,
  "Total Cost": 52438.28,
  "Plan Rows": 710,
  "Plan Width": 1775,
  "Actual Startup Time": 0.171,
  "Actual Total Time": 145.067,
  "Actual Rows": 10000,
  "Actual Loops": 1,
  "Inner Unique": false,
  "Shared Hit Blocks": 30280,
  "Shared Read Blocks": 0,
  "Shared Dirtied Blocks": 0,
  "Shared Written Blocks": 0,
  "Local Hit Blocks": 0,
  "Local Read Blocks": 0,
  "Local Dirtied Blocks": 0,
  "Local Written Blocks": 0,
  "Temp Read Blocks": 0,
  "Temp Written Blocks": 0,
  "Plans": [
    {
      "Node Type": "Seq Scan",
      "Parent Relationship": "Outer",
      "Parallel Aware": false,
      "Async Capable": false,
      "Relation Name": "delivery_points",
      "Alias": "delivery_points",
      "Startup Cost": 0.0,
      "Total Cost": 1.65,
      "Plan Rows": 1,
      "Plan Width": 16,
      "Actual Startup Time": 0.021,
      "Actual Total Time": 0.025,
      "Actual Rows": 1,
      "Actual Loops": 1,
      "Filter": "((is_active IS TRUE) AND (id = '73835e92-820e-584b-8280-bb61c63aa28e'::uuid) AND (((id = '78281fb3-e726-5396-802e-34b01fda21e8'::uuid) AND ((name)::text = 'Dalian'::text) AND ((region)::text = 'Asia'::text)) OR ((id = '262d36ae-6f35-5785-b9e8-9e221b0f1b78'::uuid) AND ((name)::text = 'Busan'::text) AND ((region)::text = 'Asia'::text)) OR ((id = '633c0593-f9b9-52ad-abe5-a0285f6888d3'::uuid) AND ((name)::text = 'Shanghai'::text) AND ((region)::text = 'Asia'::text)) OR ((id = '73835e92-820e-584b-8280-bb61c63aa28e'::uuid) AND ((name)::text = 'Singapore'::text) AND ((region)::text = 'Asia'::text)) OR ((id = '1379d36c-1ca9-55b7-9c0d-5235a0ba1f36'::uuid) AND ((name)::text = 'Rotterdam'::text) AND ((region)::text = 'Europe'::text)) OR ((id = 'a083db06-b050-56c2-a274-3897eac2fdae'::uuid) AND ((name)::text = 'Houston'::text) AND ((region)::text = 'Americas'::text)) OR ((id = '1be2a5cb-fc34-5e88-8b15-7f7d0fd6e534'::uuid) AND ((name)::text = 'Los Angeles'::text) AND ((region)::text = 'Americas'::text)) OR ((id = 'a4762444-4e0b-5bc1-b647-a185047adae5'::uuid) AND ((name)::text = 'Santos'::text) AND ((region)::text = 'Americas'::text))))",
      "Rows Removed by Filter": 8,
      "Shared Hit Blocks": 1,
      "Shared Read Blocks": 0,
      "Shared Dirtied Blocks": 0,
      "Shared Written Blocks": 0,
      "Local Hit Blocks": 0,
      "Local Read Blocks": 0,
      "Local Dirtied Blocks": 0,
      "Local Written Blocks": 0,
      "Temp Read Blocks": 0,
      "Temp Written Blocks": 0
    },
    {
      "Node Type": "Nested Loop",
      "Parent Relationship": "Inner",
      "Parallel Aware": false,
      "Async Capable": false,
      "Join Type": "Inner",
      "Startup Cost": 0.29,
      "Total Cost": 52386.92,
      "Plan Rows": 710,
      "Plan Width": 1759,
      "Actual Startup Time": 0.143,
      "Actual Total Time": 138.351,
      "Actual Rows": 10000,
      "Actual Loops": 1,
      "Inner Unique": false,
      "Shared Hit Blocks": 30279,
      "Shared Read Blocks": 0,
      "Shared Dirtied Blocks": 0,
      "Shared Written Blocks": 0,
      "Local Hit Blocks": 0,
      "Local Read Blocks": 0,
      "Local Dirtied Blocks": 0,
      "Local Written Blocks": 0,
      "Temp Read Blocks": 0,
      "Temp Written Blocks": 0,
      "Plans": [
        {
          "Node Type": "Seq Scan",
          "Parent Relationship": "Outer",
          "Parallel Aware": false,
          "Async Capable": false,
          "Relation Name": "products",
          "Alias": "products",
          "Startup Cost": 0.0,
          "Total Cost": 1.93,
          "Plan Rows": 1,
          "Plan Width": 41,
          "Actual Startup Time": 0.012,
          "Actual Total Time": 0.036,
          "Actual Rows": 1,
          "Actual Loops": 1,
          "Filter": "((is_active IS TRUE) AND (((id = '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid) AND ((name)::text = 'Bio-LNG'::text) AND ((fuel_type)::text = 'LNG'::text) AND ((fuel_grade)::text = 'Bio'::text)) OR ((id = 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) AND ((name)::text = 'UCOME B100'::text) AND ((fuel_type)::text = 'FAME'::text) AND ((fuel_grade)::text = 'UCOME'::text)) OR ((id = '9510c713-6e39-5080-add3-0a7c29657b79'::uuid) AND ((name)::text = 'Bio Methanol'::text) AND ((fuel_type)::text = 'Methanol'::text) AND ((fuel_grade)::text = 'Bio'::text)) OR ((id = 'f9b20492-b445-59cd-b292-a386d913f488'::uuid) AND ((name)::text = 'e-Methanol'::text) AND ((fuel_type)::text = 'Methanol'::text) AND ((fuel_grade)::text = 'E'::text)) OR ((id = 'c4a688be-f7c2-5edc-8f93-6b34e387609c'::uuid) AND ((name)::text = 'Bio Ethanol'::text) AND ((fuel_type)::text = 'Ethanol'::text) AND ((fuel_grade)::text = 'Bio'::text)) OR ((id = 'd186bffb-766d-5944-8825-989abbdcfc46'::uuid) AND ((name)::text = 'Synthetic Ethanol'::text) AND ((fuel_type)::text = 'Ethanol'::text) AND ((fuel_grade)::text = 'Synthetic'::text))) AND ((CASE WHEN ((id = '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid) AND ((name)::text = 'Bio-LNG'::text) AND ((fuel_type)::text = 'LNG'::text) AND ((fuel_grade)::text = 'Bio'::text)) THEN 'BIO_LNG'::character varying WHEN ((id = 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) AND ((name)::text = 'UCOME B100'::text) AND ((fuel_type)::text = 'FAME'::text) AND ((fuel_grade)::text = 'UCOME'::text)) THEN 'UCOME_B100'::character varying WHEN ((id = '9510c713-6e39-5080-add3-0a7c29657b79'::uuid) AND ((name)::text = 'Bio Methanol'::text) AND ((fuel_type)::text = 'Methanol'::text) AND ((fuel_grade)::text = 'Bio'::text)) THEN 'BIO_METHANOL'::character varying WHEN ((id = 'f9b20492-b445-59cd-b292-a386d913f488'::uuid) AND ((name)::text = 'e-Methanol'::text) AND ((fuel_type)::text = 'Methanol'::text) AND ((fuel_grade)::text = 'E'::text)) THEN 'E_METHANOL'::character varying WHEN ((id = 'c4a688be-f7c2-5edc-8f93-6b34e387609c'::uuid) AND ((name)::text = 'Bio Ethanol'::text) AND ((fuel_type)::text = 'Ethanol'::text) AND ((fuel_grade)::text = 'Bio'::text)) THEN 'BIO_ETHANOL'::character varying WHEN ((id = 'd186bffb-766d-5944-8825-989abbdcfc46'::uuid) AND ((name)::text = 'Synthetic Ethanol'::text) AND ((fuel_type)::text = 'Ethanol'::text) AND ((fuel_grade)::text = 'Synthetic'::text)) THEN 'SYNTHETIC_ETHANOL'::character varying ELSE NULL::character varying END)::text = 'BIO_METHANOL'::text))",
          "Rows Removed by Filter": 6,
          "Shared Hit Blocks": 1,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0
        },
        {
          "Node Type": "Index Scan",
          "Parent Relationship": "Inner",
          "Parallel Aware": false,
          "Async Capable": false,
          "Scan Direction": "Forward",
          "Index Name": "ix_orderbook_orders_public_aggregate",
          "Relation Name": "orderbook_orders",
          "Alias": "orderbook_orders",
          "Startup Cost": 0.29,
          "Total Cost": 52375.06,
          "Plan Rows": 994,
          "Plan Width": 1718,
          "Actual Startup Time": 0.128,
          "Actual Total Time": 134.842,
          "Actual Rows": 10000,
          "Actual Loops": 1,
          "Index Cond": "(((status)::text = ANY ('{OPEN,PARTIALLY_FILLED}'::text[])) AND ((provenance)::text = ANY ('{REAL,DEMO}'::text[])) AND (product_id = products.id) AND (delivery_point_id = '73835e92-820e-584b-8280-bb61c63aa28e'::uuid) AND ((availability_window)::text = '2031-04'::text) AND ((side)::text = 'BID'::text))",
          "Rows Removed by Index Recheck": 0,
          "Filter": "((remaining_quantity_mt > '0'::numeric) AND ((expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone) OR ((expires_at IS NULL) AND ((provenance)::text = ANY ('{REAL,UNKNOWN}'::text[])))) AND ((product_id <> '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid) OR ((delivery_point_id = '1379d36c-1ca9-55b7-9c0d-5235a0ba1f36'::uuid) AND ((lng_terms ->> 'side'::text) = (side)::text) AND (off_spec IS FALSE))) AND ((product_id <> 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid) OR ((delivery_point_id = '73835e92-820e-584b-8280-bb61c63aa28e'::uuid) AND ((fame_terms ->> 'side'::text) = (side)::text) AND (off_spec IS FALSE) AND (((side)::text <> 'ASK'::text) OR (((fame_terms ->> 'certificate_valid_until'::text) >= '2026-10-15'::text) AND (msds_available IS TRUE) AND (certification_declared IS TRUE) AND (length(TRIM(BOTH FROM COALESCE(certification_scheme, ''::character varying))) > 0))))) AND CASE WHEN ((creation_method)::text = 'MARKET_SUPPORT'::text) THEN EXISTS(SubPlan 5) ELSE ((owner_user_id IS NULL) OR EXISTS(SubPlan 9)) END)",
          "Rows Removed by Filter": 0,
          "Shared Hit Blocks": 30278,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0,
          "Plans": [
            {
              "Node Type": "Result",
              "Parent Relationship": "SubPlan",
              "Subplan Name": "SubPlan 5",
              "Parallel Aware": false,
              "Async Capable": false,
              "Startup Cost": 14.05,
              "Total Cost": 15.45,
              "Plan Rows": 1,
              "Plan Width": 0,
              "Actual Startup Time": 0.0,
              "Actual Total Time": 0.0,
              "Actual Rows": 0,
              "Actual Loops": 0,
              "One-Time Filter": "(((orderbook_orders.creation_method)::text = 'MARKET_SUPPORT'::text) AND (InitPlan 3).col1 AND (InitPlan 4).col1 AND (orderbook_orders.owner_user_id = orderbook_orders.created_by_actor_user_id))",
              "Shared Hit Blocks": 0,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0,
              "Plans": [
                {
                  "Node Type": "Index Scan",
                  "Parent Relationship": "InitPlan",
                  "Subplan Name": "InitPlan 3",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Scan Direction": "Forward",
                  "Index Name": "ix_users_organization_id_id",
                  "Relation Name": "users",
                  "Alias": "users_1",
                  "Startup Cost": 0.14,
                  "Total Cost": 12.82,
                  "Plan Rows": 1,
                  "Plan Width": 0,
                  "Actual Startup Time": 0.0,
                  "Actual Total Time": 0.0,
                  "Actual Rows": 0,
                  "Actual Loops": 0,
                  "Index Cond": "(organization_id = orderbook_orders.organization_id)",
                  "Rows Removed by Index Recheck": 0,
                  "Filter": "((email_verified IS TRUE) AND (must_change_password IS FALSE) AND ((role)::text = ANY ('{BUYER,SUPPLIER}'::text[])) AND ((kyc_status)::text <> 'REJECTED'::text) AND ((status)::text = 'APPROVED'::text) AND ((((kyc_status)::text = 'APPROVED'::text) AND (kyc_organization_id = organization_id)) OR (ANY ((id = (hashed SubPlan 2).col1) AND (organization_id = (hashed SubPlan 2).col2)))))",
                  "Rows Removed by Filter": 0,
                  "Shared Hit Blocks": 0,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0,
                  "Plans": [
                    {
                      "Node Type": "Seq Scan",
                      "Parent Relationship": "SubPlan",
                      "Subplan Name": "SubPlan 2",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Relation Name": "kyc_execution_grace",
                      "Alias": "kyc_execution_grace",
                      "Startup Cost": 0.0,
                      "Total Cost": 1.1,
                      "Plan Rows": 6,
                      "Plan Width": 32,
                      "Actual Startup Time": 0.0,
                      "Actual Total Time": 0.0,
                      "Actual Rows": 0,
                      "Actual Loops": 0,
                      "Filter": "(expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone)",
                      "Rows Removed by Filter": 0,
                      "Shared Hit Blocks": 0,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0
                    }
                  ]
                },
                {
                  "Node Type": "Seq Scan",
                  "Parent Relationship": "InitPlan",
                  "Subplan Name": "InitPlan 4",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Relation Name": "organizations",
                  "Alias": "organizations",
                  "Startup Cost": 0.0,
                  "Total Cost": 1.23,
                  "Plan Rows": 1,
                  "Plan Width": 0,
                  "Actual Startup Time": 0.0,
                  "Actual Total Time": 0.0,
                  "Actual Rows": 0,
                  "Actual Loops": 0,
                  "Filter": "((id = orderbook_orders.organization_id) AND ((verification_status)::text = 'APPROVED'::text) AND ((provenance)::text = 'REAL'::text))",
                  "Rows Removed by Filter": 0,
                  "Shared Hit Blocks": 0,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0
                },
                {
                  "Node Type": "Seq Scan",
                  "Parent Relationship": "Outer",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Relation Name": "users",
                  "Alias": "users",
                  "Startup Cost": 14.05,
                  "Total Cost": 15.45,
                  "Plan Rows": 1,
                  "Plan Width": 0,
                  "Actual Startup Time": 0.0,
                  "Actual Total Time": 0.0,
                  "Actual Rows": 0,
                  "Actual Loops": 0,
                  "Filter": "((id = orderbook_orders.created_by_actor_user_id) AND ((role)::text = 'ADMIN'::text) AND ((status)::text = 'APPROVED'::text))",
                  "Rows Removed by Filter": 0,
                  "Shared Hit Blocks": 0,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0
                }
              ]
            },
            {
              "Node Type": "Result",
              "Parent Relationship": "SubPlan",
              "Subplan Name": "SubPlan 9",
              "Parallel Aware": false,
              "Async Capable": false,
              "Startup Cost": 1.33,
              "Total Cost": 10.5,
              "Plan Rows": 1,
              "Plan Width": 0,
              "Actual Startup Time": 0.01,
              "Actual Total Time": 0.01,
              "Actual Rows": 1,
              "Actual Loops": 10000,
              "One-Time Filter": "(InitPlan 8).col1",
              "Shared Hit Blocks": 30000,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0,
              "Plans": [
                {
                  "Node Type": "Seq Scan",
                  "Parent Relationship": "InitPlan",
                  "Subplan Name": "InitPlan 8",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Relation Name": "organizations",
                  "Alias": "organizations_1",
                  "Startup Cost": 0.0,
                  "Total Cost": 1.2,
                  "Plan Rows": 1,
                  "Plan Width": 0,
                  "Actual Startup Time": 0.002,
                  "Actual Total Time": 0.002,
                  "Actual Rows": 1,
                  "Actual Loops": 10000,
                  "Filter": "((id = orderbook_orders.organization_id) AND ((verification_status)::text = 'APPROVED'::text))",
                  "Rows Removed by Filter": 3,
                  "Shared Hit Blocks": 10000,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0
                },
                {
                  "Node Type": "Index Scan",
                  "Parent Relationship": "Outer",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Scan Direction": "Forward",
                  "Index Name": "ix_users_organization_id_id",
                  "Relation Name": "users",
                  "Alias": "users_2",
                  "Startup Cost": 1.33,
                  "Total Cost": 10.5,
                  "Plan Rows": 1,
                  "Plan Width": 0,
                  "Actual Startup Time": 0.007,
                  "Actual Total Time": 0.007,
                  "Actual Rows": 1,
                  "Actual Loops": 10000,
                  "Index Cond": "((organization_id = orderbook_orders.organization_id) AND (id = orderbook_orders.owner_user_id))",
                  "Rows Removed by Index Recheck": 0,
                  "Filter": "((email_verified IS TRUE) AND (must_change_password IS FALSE) AND ((role)::text = ANY ('{BUYER,SUPPLIER}'::text[])) AND ((kyc_status)::text <> 'REJECTED'::text) AND ((status)::text = 'APPROVED'::text) AND ((((kyc_status)::text = 'APPROVED'::text) AND (kyc_organization_id = organization_id)) OR (ANY ((id = (hashed SubPlan 7).col1) AND (organization_id = (hashed SubPlan 7).col2)))))",
                  "Rows Removed by Filter": 0,
                  "Shared Hit Blocks": 20000,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0,
                  "Plans": [
                    {
                      "Node Type": "Seq Scan",
                      "Parent Relationship": "SubPlan",
                      "Subplan Name": "SubPlan 7",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Relation Name": "kyc_execution_grace",
                      "Alias": "kyc_execution_grace_1",
                      "Startup Cost": 0.0,
                      "Total Cost": 1.1,
                      "Plan Rows": 6,
                      "Plan Width": 32,
                      "Actual Startup Time": 0.0,
                      "Actual Total Time": 0.0,
                      "Actual Rows": 0,
                      "Actual Loops": 0,
                      "Filter": "(expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone)",
                      "Rows Removed by Filter": 0,
                      "Shared Hit Blocks": 0,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0
                    }
                  ]
                }
              ]
            }
          ]
        }
      ]
    }
  ]
}
```

### rust /api/orderbook/bids rows=1

```sql
WITH eligible_orders AS (-- Shared public-order visibility projection (Task 6).
--
-- This is the complete bound internal relation: every caller value is a parameter, so
-- no route ever concatenates text into SQL. Parameters are
--   $1  attested market policy (jsonb text)
--   $2  captured UTC instant (timestamptz)
--   $3  Singapore date (text, YYYY-MM-DD)
--   $4  include_off_spec (boolean)
--   $5  product_id (uuid, null = any)
--   $6  delivery_point_id (uuid, null = any)
--   $7  side (text, null = any)
--   $8  fuel_type (text, null = any)
--   $9  market_product code (text, null = any)
--   $10 region (text, null = any)
--   $11 availability_window (text, null = any)
--   $12 surface: orderbook, listings, or candidates (text)
--   $13 runtime KYC execution gate enabled (boolean)
-- No pagination here: consumers rank, aggregate and page the shared relation.
--
-- Product identity comes only from the policy. The statement never names a
-- product, a product code, or a product UUID.
WITH policy_products AS (
    SELECT *
    FROM jsonb_to_recordset(($1::jsonb) -> 'products') AS policy_product(
        id uuid,
        code text,
        name text,
        fuel_type text,
        fuel_grade text,
        execution_mode text,
        lanes jsonb,
        qualification_profile text,
        display_order integer
    )
),
policy_points AS (
    SELECT *
    FROM jsonb_to_recordset(($1::jsonb) -> 'points') AS policy_point(
        id uuid,
        name text,
        region text,
        timezone text,
        display_order integer
    )
)
SELECT o.*, p.name AS product_name, p.fuel_type AS product_fuel_type,
       p.fuel_grade AS product_fuel_grade, p.is_active AS product_is_active,
       dp.name AS delivery_point_name, dp.region AS delivery_point_region,
       (SELECT pp.code FROM policy_products pp WHERE pp.id = p.id) AS market_product,
       -- Internal only. Never serialize this object as a public DTO.
       to_jsonb(o) || jsonb_build_object(
           'quantity_mt', o.quantity_mt::text,
           'remaining_quantity_mt', o.remaining_quantity_mt::text,
           'price_per_mt_usd', o.price_per_mt_usd::text,
           'carbon_intensity_gco2_mj', o.carbon_intensity_gco2_mj::text,
           'energy_density_mj_kg', o.energy_density_mj_kg::text
       ) AS private_digest_input
FROM orderbook_orders AS o
JOIN products AS p ON p.id = o.product_id
JOIN delivery_points AS dp ON dp.id = o.delivery_point_id
WHERE o.status IN ('OPEN', 'PARTIALLY_FILLED')
    -- Strict canonical product identity, active row, and approved lane.
    AND p.is_active
    AND EXISTS (
        SELECT 1
        FROM policy_products AS pp
        WHERE pp.execution_mode = 'ORDERBOOK'
          AND pp.id = p.id
          AND pp.name = p.name
          AND pp.fuel_type = p.fuel_type
          AND pp.fuel_grade = p.fuel_grade
          AND EXISTS (
              SELECT 1
              FROM jsonb_array_elements_text(pp.lanes) AS lane(value)
              WHERE lane.value = o.delivery_point_id::text
          )
    )
    -- Strict canonical delivery-point identity and active row.
    AND dp.is_active
    AND EXISTS (
        SELECT 1
        FROM policy_points AS pq
        WHERE pq.id = dp.id AND pq.name = dp.name AND pq.region = dp.region
    )
    -- Canonical availability-window grammar.
    AND ($12::text <> 'orderbook' OR (
        o.availability_window = 'SPOT'
        OR o.availability_window ~ '^[0-9]{4}-(0[1-9]|1[0-2])$'
        OR o.availability_window ~ '^[0-9]{4}-Q[1-4]$'
        OR o.availability_window ~ '^[0-9]{4}-CAL$'
    ))
    AND (-- Shared expiry, owner admission and shallow declaration rule.
-- Requires policy_products and alias o. Binds $2, $3 and $13.
TRUE
    -- Currency: an explicit future expiry, or a null expiry on REAL/UNKNOWN.
    AND (
        o.expires_at > $2::timestamptz
        OR (
            o.expires_at IS NULL
            AND o.provenance IN ('REAL', 'UNKNOWN')
        )
    )
    -- Owner admission. Assisted rows use the narrower administrator rule.
    AND (
        CASE
            WHEN o.creation_method = 'MARKET_SUPPORT' THEN EXISTS (
                SELECT 1
                FROM users AS actor
                WHERE actor.id = o.owner_user_id
                  AND actor.id = o.created_by_actor_user_id
                  AND actor.role = 'ADMIN'
                  AND actor.status = 'APPROVED'
                  AND EXISTS (
                      SELECT 1
                      FROM users AS member
                      WHERE member.organization_id = o.organization_id
                        AND member.role IN ('BUYER', 'SUPPLIER')
                        AND member.status = 'APPROVED'
                        AND member.email_verified
                        AND NOT member.must_change_password
                        AND member.kyc_status <> 'REJECTED'
                        AND CASE WHEN $13::boolean THEN (
                            (member.kyc_status = 'APPROVED'
                             AND member.kyc_organization_id = member.organization_id)
                            OR EXISTS (
                                SELECT 1 FROM kyc_execution_grace AS grace
                                WHERE grace.user_id = member.id
                                  AND grace.organization_id = member.organization_id
                                  AND grace.expires_at > $2::timestamptz
                            )
                        ) ELSE (
                            member.kyc_organization_id IS NULL
                            OR member.kyc_organization_id = member.organization_id
                        ) END
                  )
                  AND EXISTS (
                      SELECT 1
                      FROM organizations AS assisted_org
                      WHERE assisted_org.id = o.organization_id
                        AND assisted_org.verification_status = 'APPROVED'
                        AND assisted_org.provenance = 'REAL'
                  )
            )
            ELSE (
                o.owner_user_id IS NULL
                OR EXISTS (
                    SELECT 1
                    FROM users AS owner
                    WHERE owner.id = o.owner_user_id
                      AND owner.organization_id = o.organization_id
                      AND owner.role IN ('BUYER', 'SUPPLIER')
                      AND owner.status = 'APPROVED'
                      AND owner.email_verified
                      AND NOT owner.must_change_password
                      AND owner.kyc_status <> 'REJECTED'
                      AND CASE WHEN $13::boolean THEN (
                          (owner.kyc_status = 'APPROVED'
                           AND owner.kyc_organization_id = owner.organization_id)
                          OR EXISTS (
                              SELECT 1 FROM kyc_execution_grace AS grace
                              WHERE grace.user_id = owner.id
                                AND grace.organization_id = owner.organization_id
                                AND grace.expires_at > $2::timestamptz
                          )
                      ) ELSE (
                          owner.kyc_organization_id IS NULL
                          OR owner.kyc_organization_id = owner.organization_id
                      ) END
                      AND EXISTS (
                          SELECT 1
                          FROM organizations AS org
                          WHERE org.id = o.organization_id
                            AND org.verification_status = 'APPROVED'
                      )
                )
            )
        END
    )
    -- fame_v1 declaration currency, keyed by policy profile.
    AND (
        NOT EXISTS (
            SELECT 1
            FROM policy_products AS pf
            WHERE pf.qualification_profile = 'fame_v1'
              AND pf.id = o.product_id
        )
        OR (
            EXISTS (
                SELECT 1
                FROM policy_products AS pf
                WHERE pf.qualification_profile = 'fame_v1'
                  AND pf.id = o.product_id
                  AND EXISTS (
                      SELECT 1
                      FROM jsonb_array_elements_text(pf.lanes) AS lane(value)
                      WHERE lane.value = o.delivery_point_id::text
                  )
            )
            AND o.fame_terms ->> 'side' = o.side
            AND o.off_spec = false
            AND (
                o.side <> 'ASK'
                OR (
                    o.fame_terms ->> 'certificate_valid_until' >= $3::text
                    AND o.msds_available
                    AND o.certification_declared
                    AND length(btrim(coalesce(o.certification_scheme, ''))) > 0
                )
            )
        )
    )
    -- lng_v1 declaration currency, keyed by policy profile.
    AND (
        NOT EXISTS (
            SELECT 1
            FROM policy_products AS pl
            WHERE pl.qualification_profile = 'lng_v1'
              AND pl.id = o.product_id
        )
        OR (
            EXISTS (
                SELECT 1
                FROM policy_products AS pl
                WHERE pl.qualification_profile = 'lng_v1'
                  AND pl.id = o.product_id
                  AND EXISTS (
                      SELECT 1
                      FROM jsonb_array_elements_text(pl.lanes) AS lane(value)
                      WHERE lane.value = o.delivery_point_id::text
                  )
            )
            AND o.lng_terms ->> 'side' = o.side
            AND o.off_spec = false
        )
    )
)
    -- Independently labelled public rows.
    AND o.provenance IN ('REAL', 'DEMO')
    -- Admitted fuel types.
    AND p.fuel_type IN ('Methanol', 'Ethanol', 'FAME', 'LNG')
    -- Scalar ASK quality declarations.
    AND ($12::text <> 'orderbook' OR (
        o.side <> 'ASK'
        OR (
            length(btrim(coalesce(o.certification_scheme, ''))) > 0
            AND o.certification_declared
            AND length(btrim(coalesce(o.specification_standard, ''))) > 0
            AND o.msds_available
            AND (
                EXISTS (
                    SELECT 1
                    FROM policy_products AS pd
                    WHERE pd.qualification_profile IN ('fame_v1', 'lng_v1')
                      AND pd.id = p.id
                      AND pd.name = p.name
                      AND pd.fuel_type = p.fuel_type
                      AND pd.fuel_grade = p.fuel_grade
                )
                OR o.carbon_intensity_gco2_mj IS NOT NULL
            )
            AND length(btrim(coalesce(o.feedstock, ''))) > 0
            AND (
                EXISTS (
                    SELECT 1
                    FROM policy_products AS po
                    WHERE po.qualification_profile = 'fame_v1'
                      AND po.id = p.id
                      AND po.name = p.name
                      AND po.fuel_type = p.fuel_type
                      AND po.fuel_grade = p.fuel_grade
                )
                OR length(btrim(coalesce(o.origin, ''))) > 0
            )
        )
    )
    )
    -- Off-spec rows are hidden unless the caller asks for them.
    AND ($12::text <> 'orderbook' OR o.off_spec = false OR $4::boolean)
    AND ($12::text <> 'listings' OR o.side = 'ASK')
    AND $12::text IN ('orderbook', 'listings', 'candidates')
    -- Route filters.
    AND ($5::uuid IS NULL OR o.product_id = $5::uuid)
    AND ($6::uuid IS NULL OR o.delivery_point_id = $6::uuid)
    AND ($7::text IS NULL OR o.side = $7::text)
    AND ($8::text IS NULL OR p.fuel_type = $8::text)
    AND (
        $9::text IS NULL
        OR EXISTS (
            SELECT 1
            FROM policy_products AS pm
            WHERE pm.code = $9::text
              AND pm.id = p.id
              AND pm.name = p.name
              AND pm.fuel_type = p.fuel_type
              AND pm.fuel_grade = p.fuel_grade
        )
    )
    AND ($10::text IS NULL OR $10::text = '' OR dp.region = $10::text OR dp.name = $10::text)
    AND ($11::text IS NULL OR o.availability_window = $11::text)
) SELECT count(*) AS total FROM eligible_orders
```

```json
{
  "Node Type": "Aggregate",
  "Strategy": "Plain",
  "Partial Mode": "Simple",
  "Parallel Aware": false,
  "Async Capable": false,
  "Startup Cost": 3937179.46,
  "Total Cost": 3937179.47,
  "Plan Rows": 1,
  "Plan Width": 8,
  "Actual Startup Time": 264.897,
  "Actual Total Time": 264.909,
  "Actual Rows": 1,
  "Actual Loops": 1,
  "Shared Hit Blocks": 30398,
  "Shared Read Blocks": 0,
  "Shared Dirtied Blocks": 0,
  "Shared Written Blocks": 0,
  "Local Hit Blocks": 0,
  "Local Read Blocks": 0,
  "Local Dirtied Blocks": 0,
  "Local Written Blocks": 0,
  "Temp Read Blocks": 0,
  "Temp Written Blocks": 0,
  "Plans": [
    {
      "Node Type": "Nested Loop",
      "Parent Relationship": "Outer",
      "Parallel Aware": false,
      "Async Capable": false,
      "Join Type": "Semi",
      "Startup Cost": 1.0,
      "Total Cost": 3937179.45,
      "Plan Rows": 1,
      "Plan Width": 3651,
      "Actual Startup Time": 0.779,
      "Actual Total Time": 262.105,
      "Actual Rows": 10023,
      "Actual Loops": 1,
      "Inner Unique": false,
      "Join Filter": "((policy_point.id = dp.id) AND ((dp.name)::text = policy_point.name) AND (policy_point.region = (dp.region)::text))",
      "Rows Removed by Join Filter": 30069,
      "Shared Hit Blocks": 30398,
      "Shared Read Blocks": 0,
      "Shared Dirtied Blocks": 0,
      "Shared Written Blocks": 0,
      "Local Hit Blocks": 0,
      "Local Read Blocks": 0,
      "Local Dirtied Blocks": 0,
      "Local Written Blocks": 0,
      "Temp Read Blocks": 0,
      "Temp Written Blocks": 0,
      "Plans": [
        {
          "Node Type": "Function Scan",
          "Parent Relationship": "InitPlan",
          "Subplan Name": "CTE policy_products",
          "Parallel Aware": false,
          "Async Capable": false,
          "Function Name": "jsonb_to_recordset",
          "Alias": "policy_product",
          "Startup Cost": 0.0,
          "Total Cost": 1.0,
          "Plan Rows": 100,
          "Plan Width": 244,
          "Actual Startup Time": 0.065,
          "Actual Total Time": 0.067,
          "Actual Rows": 6,
          "Actual Loops": 1,
          "Shared Hit Blocks": 0,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0
        },
        {
          "Node Type": "Nested Loop",
          "Parent Relationship": "Outer",
          "Parallel Aware": false,
          "Async Capable": false,
          "Join Type": "Semi",
          "Startup Cost": 0.0,
          "Total Cost": 3937175.69,
          "Plan Rows": 1,
          "Plan Width": 47,
          "Actual Startup Time": 0.721,
          "Actual Total Time": 233.5,
          "Actual Rows": 10023,
          "Actual Loops": 1,
          "Inner Unique": false,
          "Join Filter": "((pp.id = p.id) AND ((p.name)::text = pp.name) AND (pp.fuel_type = (p.fuel_type)::text) AND (pp.fuel_grade = (p.fuel_grade)::text) AND EXISTS(SubPlan 23))",
          "Rows Removed by Join Filter": 20058,
          "Shared Hit Blocks": 30398,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0,
          "Plans": [
            {
              "Node Type": "Nested Loop",
              "Parent Relationship": "Outer",
              "Parallel Aware": false,
              "Async Capable": false,
              "Join Type": "Inner",
              "Startup Cost": 0.0,
              "Total Cost": 3935472.95,
              "Plan Rows": 954,
              "Plan Width": 103,
              "Actual Startup Time": 0.663,
              "Actual Total Time": 162.787,
              "Actual Rows": 10025,
              "Actual Loops": 1,
              "Inner Unique": true,
              "Join Filter": "((o.product_id = p.id) AND (((o.side)::text <> 'ASK'::text) OR ((length(btrim((COALESCE(o.certification_scheme, ''::character varying))::text)) > 0) AND o.certification_declared AND (length(btrim((COALESCE(o.specification_standard, ''::character varying))::text)) > 0) AND o.msds_available AND ((ANY ((p.id = (hashed SubPlan 20).col1) AND ((p.name)::text = (hashed SubPlan 20).col2) AND ((p.fuel_type)::text = (hashed SubPlan 20).col3) AND ((p.fuel_grade)::text = (hashed SubPlan 20).col4))) OR (o.carbon_intensity_gco2_mj IS NOT NULL)) AND (length(btrim((COALESCE(o.feedstock, ''::character varying))::text)) > 0) AND ((ANY ((p.id = (hashed SubPlan 22).col1) AND ((p.name)::text = (hashed SubPlan 22).col2) AND ((p.fuel_type)::text = (hashed SubPlan 22).col3) AND ((p.fuel_grade)::text = (hashed SubPlan 22).col4))) OR (length(btrim((COALESCE(o.origin, ''::character varying))::text)) > 0)))))",
              "Rows Removed by Join Filter": 10035,
              "Shared Hit Blocks": 30398,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0,
              "Plans": [
                {
                  "Node Type": "Nested Loop",
                  "Parent Relationship": "Outer",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Join Type": "Inner",
                  "Startup Cost": 0.0,
                  "Total Cost": 3896091.42,
                  "Plan Rows": 1116,
                  "Plan Width": 120,
                  "Actual Startup Time": 0.628,
                  "Actual Total Time": 153.433,
                  "Actual Rows": 10026,
                  "Actual Loops": 1,
                  "Inner Unique": true,
                  "Join Filter": "(o.delivery_point_id = dp.id)",
                  "Rows Removed by Join Filter": 15,
                  "Shared Hit Blocks": 30397,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0,
                  "Plans": [
                    {
                      "Node Type": "Seq Scan",
                      "Parent Relationship": "Outer",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Relation Name": "orderbook_orders",
                      "Alias": "o",
                      "Startup Cost": 0.0,
                      "Total Cost": 3895952.69,
                      "Plan Rows": 1255,
                      "Plan Width": 89,
                      "Actual Startup Time": 0.604,
                      "Actual Total Time": 129.481,
                      "Actual Rows": 10027,
                      "Actual Loops": 1,
                      "Filter": "((NOT off_spec) AND ((status)::text = ANY ('{OPEN,PARTIALLY_FILLED}'::text[])) AND ((provenance)::text = ANY ('{REAL,DEMO}'::text[])) AND ((side)::text = 'BID'::text) AND ((expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone) OR ((expires_at IS NULL) AND ((provenance)::text = ANY ('{REAL,UNKNOWN}'::text[])))) AND (((availability_window)::text = 'SPOT'::text) OR ((availability_window)::text ~ '^[0-9]{4}-(0[1-9]|1[0-2])$'::text) OR ((availability_window)::text ~ '^[0-9]{4}-Q[1-4]$'::text) OR ((availability_window)::text ~ '^[0-9]{4}-CAL$'::text)) AND CASE WHEN ((creation_method)::text = 'MARKET_SUPPORT'::text) THEN EXISTS(SubPlan 6) ELSE ((owner_user_id IS NULL) OR EXISTS(SubPlan 10)) END AND ((NOT (ANY (product_id = (hashed SubPlan 16).col1))) OR (EXISTS(SubPlan 18) AND ((lng_terms ->> 'side'::text) = (side)::text) AND (NOT off_spec))) AND ((NOT (ANY (product_id = (hashed SubPlan 12).col1))) OR (EXISTS(SubPlan 14) AND ((fame_terms ->> 'side'::text) = (side)::text) AND (NOT off_spec) AND (((side)::text <> 'ASK'::text) OR (((fame_terms ->> 'certificate_valid_until'::text) >= '2026-10-15'::text) AND msds_available AND certification_declared AND (length(btrim((COALESCE(certification_scheme, ''::character varying))::text)) > 0))))))",
                      "Rows Removed by Filter": 52,
                      "Shared Hit Blocks": 30396,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0,
                      "Plans": [
                        {
                          "Node Type": "Result",
                          "Parent Relationship": "SubPlan",
                          "Subplan Name": "SubPlan 6",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Startup Cost": 14.05,
                          "Total Cost": 15.45,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.022,
                          "Actual Total Time": 0.023,
                          "Actual Rows": 0,
                          "Actual Loops": 7,
                          "One-Time Filter": "((InitPlan 4).col1 AND (InitPlan 5).col1 AND (o.owner_user_id = o.created_by_actor_user_id))",
                          "Shared Hit Blocks": 22,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0,
                          "Plans": [
                            {
                              "Node Type": "Index Scan",
                              "Parent Relationship": "InitPlan",
                              "Subplan Name": "InitPlan 4",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Scan Direction": "Forward",
                              "Index Name": "ix_users_organization_id_id",
                              "Relation Name": "users",
                              "Alias": "member",
                              "Startup Cost": 0.14,
                              "Total Cost": 12.82,
                              "Plan Rows": 1,
                              "Plan Width": 0,
                              "Actual Startup Time": 0.014,
                              "Actual Total Time": 0.014,
                              "Actual Rows": 1,
                              "Actual Loops": 7,
                              "Index Cond": "(organization_id = o.organization_id)",
                              "Rows Removed by Index Recheck": 0,
                              "Filter": "(email_verified AND (NOT must_change_password) AND ((role)::text = ANY ('{BUYER,SUPPLIER}'::text[])) AND ((kyc_status)::text <> 'REJECTED'::text) AND ((status)::text = 'APPROVED'::text) AND ((((kyc_status)::text = 'APPROVED'::text) AND (kyc_organization_id = organization_id)) OR (ANY ((id = (hashed SubPlan 3).col1) AND (organization_id = (hashed SubPlan 3).col2)))))",
                              "Rows Removed by Filter": 0,
                              "Shared Hit Blocks": 15,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0,
                              "Plans": [
                                {
                                  "Node Type": "Seq Scan",
                                  "Parent Relationship": "SubPlan",
                                  "Subplan Name": "SubPlan 3",
                                  "Parallel Aware": false,
                                  "Async Capable": false,
                                  "Relation Name": "kyc_execution_grace",
                                  "Alias": "grace",
                                  "Startup Cost": 0.0,
                                  "Total Cost": 1.1,
                                  "Plan Rows": 6,
                                  "Plan Width": 32,
                                  "Actual Startup Time": 0.004,
                                  "Actual Total Time": 0.006,
                                  "Actual Rows": 6,
                                  "Actual Loops": 1,
                                  "Filter": "(expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone)",
                                  "Rows Removed by Filter": 2,
                                  "Shared Hit Blocks": 1,
                                  "Shared Read Blocks": 0,
                                  "Shared Dirtied Blocks": 0,
                                  "Shared Written Blocks": 0,
                                  "Local Hit Blocks": 0,
                                  "Local Read Blocks": 0,
                                  "Local Dirtied Blocks": 0,
                                  "Local Written Blocks": 0,
                                  "Temp Read Blocks": 0,
                                  "Temp Written Blocks": 0
                                }
                              ]
                            },
                            {
                              "Node Type": "Seq Scan",
                              "Parent Relationship": "InitPlan",
                              "Subplan Name": "InitPlan 5",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Relation Name": "organizations",
                              "Alias": "assisted_org",
                              "Startup Cost": 0.0,
                              "Total Cost": 1.23,
                              "Plan Rows": 1,
                              "Plan Width": 0,
                              "Actual Startup Time": 0.005,
                              "Actual Total Time": 0.005,
                              "Actual Rows": 1,
                              "Actual Loops": 4,
                              "Filter": "((id = o.organization_id) AND ((verification_status)::text = 'APPROVED'::text) AND ((provenance)::text = 'REAL'::text))",
                              "Rows Removed by Filter": 4,
                              "Shared Hit Blocks": 4,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            },
                            {
                              "Node Type": "Seq Scan",
                              "Parent Relationship": "Outer",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Relation Name": "users",
                              "Alias": "actor",
                              "Startup Cost": 14.05,
                              "Total Cost": 15.45,
                              "Plan Rows": 1,
                              "Plan Width": 0,
                              "Actual Startup Time": 0.008,
                              "Actual Total Time": 0.008,
                              "Actual Rows": 1,
                              "Actual Loops": 3,
                              "Filter": "((id = o.created_by_actor_user_id) AND ((role)::text = 'ADMIN'::text) AND ((status)::text = 'APPROVED'::text))",
                              "Rows Removed by Filter": 13,
                              "Shared Hit Blocks": 3,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            }
                          ]
                        },
                        {
                          "Node Type": "Result",
                          "Parent Relationship": "SubPlan",
                          "Subplan Name": "SubPlan 10",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Startup Cost": 1.33,
                          "Total Cost": 10.5,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.008,
                          "Actual Total Time": 0.008,
                          "Actual Rows": 1,
                          "Actual Loops": 10036,
                          "One-Time Filter": "(InitPlan 9).col1",
                          "Shared Hit Blocks": 30106,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0,
                          "Plans": [
                            {
                              "Node Type": "Seq Scan",
                              "Parent Relationship": "InitPlan",
                              "Subplan Name": "InitPlan 9",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Relation Name": "organizations",
                              "Alias": "org",
                              "Startup Cost": 0.0,
                              "Total Cost": 1.2,
                              "Plan Rows": 1,
                              "Plan Width": 0,
                              "Actual Startup Time": 0.002,
                              "Actual Total Time": 0.002,
                              "Actual Rows": 1,
                              "Actual Loops": 10036,
                              "Filter": "((id = o.organization_id) AND ((verification_status)::text = 'APPROVED'::text))",
                              "Rows Removed by Filter": 3,
                              "Shared Hit Blocks": 10036,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            },
                            {
                              "Node Type": "Index Scan",
                              "Parent Relationship": "Outer",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Scan Direction": "Forward",
                              "Index Name": "ix_users_organization_id_id",
                              "Relation Name": "users",
                              "Alias": "owner",
                              "Startup Cost": 1.33,
                              "Total Cost": 10.5,
                              "Plan Rows": 1,
                              "Plan Width": 0,
                              "Actual Startup Time": 0.003,
                              "Actual Total Time": 0.003,
                              "Actual Rows": 1,
                              "Actual Loops": 10035,
                              "Index Cond": "((organization_id = o.organization_id) AND (id = o.owner_user_id))",
                              "Rows Removed by Index Recheck": 0,
                              "Filter": "(email_verified AND (NOT must_change_password) AND ((role)::text = ANY ('{BUYER,SUPPLIER}'::text[])) AND ((kyc_status)::text <> 'REJECTED'::text) AND ((status)::text = 'APPROVED'::text) AND ((((kyc_status)::text = 'APPROVED'::text) AND (kyc_organization_id = organization_id)) OR (ANY ((id = (hashed SubPlan 8).col1) AND (organization_id = (hashed SubPlan 8).col2)))))",
                              "Rows Removed by Filter": 0,
                              "Shared Hit Blocks": 20070,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0,
                              "Plans": [
                                {
                                  "Node Type": "Seq Scan",
                                  "Parent Relationship": "SubPlan",
                                  "Subplan Name": "SubPlan 8",
                                  "Parallel Aware": false,
                                  "Async Capable": false,
                                  "Relation Name": "kyc_execution_grace",
                                  "Alias": "grace_1",
                                  "Startup Cost": 0.0,
                                  "Total Cost": 1.1,
                                  "Plan Rows": 6,
                                  "Plan Width": 32,
                                  "Actual Startup Time": 0.008,
                                  "Actual Total Time": 0.01,
                                  "Actual Rows": 6,
                                  "Actual Loops": 1,
                                  "Filter": "(expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone)",
                                  "Rows Removed by Filter": 2,
                                  "Shared Hit Blocks": 1,
                                  "Shared Read Blocks": 0,
                                  "Shared Dirtied Blocks": 0,
                                  "Shared Written Blocks": 0,
                                  "Local Hit Blocks": 0,
                                  "Local Read Blocks": 0,
                                  "Local Dirtied Blocks": 0,
                                  "Local Written Blocks": 0,
                                  "Temp Read Blocks": 0,
                                  "Temp Written Blocks": 0
                                }
                              ]
                            }
                          ]
                        },
                        {
                          "Node Type": "CTE Scan",
                          "Parent Relationship": "SubPlan",
                          "Subplan Name": "SubPlan 16",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "CTE Name": "policy_products",
                          "Alias": "pl",
                          "Startup Cost": 0.0,
                          "Total Cost": 2.25,
                          "Plan Rows": 1,
                          "Plan Width": 16,
                          "Actual Startup Time": 0.069,
                          "Actual Total Time": 0.098,
                          "Actual Rows": 1,
                          "Actual Loops": 1,
                          "Filter": "(qualification_profile = 'lng_v1'::text)",
                          "Rows Removed by Filter": 5,
                          "Shared Hit Blocks": 0,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0
                        },
                        {
                          "Node Type": "CTE Scan",
                          "Parent Relationship": "SubPlan",
                          "Subplan Name": "SubPlan 18",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "CTE Name": "policy_products",
                          "Alias": "pl_1",
                          "Startup Cost": 0.0,
                          "Total Cost": 177.75,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.0,
                          "Actual Total Time": 0.0,
                          "Actual Rows": 0,
                          "Actual Loops": 0,
                          "Filter": "((qualification_profile = 'lng_v1'::text) AND (id = o.product_id) AND EXISTS(SubPlan 17))",
                          "Rows Removed by Filter": 0,
                          "Shared Hit Blocks": 0,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0,
                          "Plans": [
                            {
                              "Node Type": "Function Scan",
                              "Parent Relationship": "SubPlan",
                              "Subplan Name": "SubPlan 17",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Function Name": "jsonb_array_elements_text",
                              "Alias": "lane_1",
                              "Startup Cost": 0.0,
                              "Total Cost": 1.75,
                              "Plan Rows": 1,
                              "Plan Width": 0,
                              "Actual Startup Time": 0.0,
                              "Actual Total Time": 0.0,
                              "Actual Rows": 0,
                              "Actual Loops": 0,
                              "Filter": "(value = (o.delivery_point_id)::text)",
                              "Rows Removed by Filter": 0,
                              "Shared Hit Blocks": 0,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            }
                          ]
                        },
                        {
                          "Node Type": "CTE Scan",
                          "Parent Relationship": "SubPlan",
                          "Subplan Name": "SubPlan 12",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "CTE Name": "policy_products",
                          "Alias": "pf",
                          "Startup Cost": 0.0,
                          "Total Cost": 2.25,
                          "Plan Rows": 1,
                          "Plan Width": 16,
                          "Actual Startup Time": 0.002,
                          "Actual Total Time": 0.003,
                          "Actual Rows": 1,
                          "Actual Loops": 1,
                          "Filter": "(qualification_profile = 'fame_v1'::text)",
                          "Rows Removed by Filter": 5,
                          "Shared Hit Blocks": 0,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0
                        },
                        {
                          "Node Type": "CTE Scan",
                          "Parent Relationship": "SubPlan",
                          "Subplan Name": "SubPlan 14",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "CTE Name": "policy_products",
                          "Alias": "pf_1",
                          "Startup Cost": 0.0,
                          "Total Cost": 177.75,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.0,
                          "Actual Total Time": 0.0,
                          "Actual Rows": 0,
                          "Actual Loops": 0,
                          "Filter": "((qualification_profile = 'fame_v1'::text) AND (id = o.product_id) AND EXISTS(SubPlan 13))",
                          "Rows Removed by Filter": 0,
                          "Shared Hit Blocks": 0,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0,
                          "Plans": [
                            {
                              "Node Type": "Function Scan",
                              "Parent Relationship": "SubPlan",
                              "Subplan Name": "SubPlan 13",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Function Name": "jsonb_array_elements_text",
                              "Alias": "lane",
                              "Startup Cost": 0.0,
                              "Total Cost": 1.75,
                              "Plan Rows": 1,
                              "Plan Width": 0,
                              "Actual Startup Time": 0.0,
                              "Actual Total Time": 0.0,
                              "Actual Rows": 0,
                              "Actual Loops": 0,
                              "Filter": "(value = (o.delivery_point_id)::text)",
                              "Rows Removed by Filter": 0,
                              "Shared Hit Blocks": 0,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            }
                          ]
                        }
                      ]
                    },
                    {
                      "Node Type": "Materialize",
                      "Parent Relationship": "Inner",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Startup Cost": 0.0,
                      "Total Cost": 1.13,
                      "Plan Rows": 8,
                      "Plan Width": 31,
                      "Actual Startup Time": 0.0,
                      "Actual Total Time": 0.0,
                      "Actual Rows": 1,
                      "Actual Loops": 10027,
                      "Shared Hit Blocks": 1,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0,
                      "Plans": [
                        {
                          "Node Type": "Seq Scan",
                          "Parent Relationship": "Outer",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Relation Name": "delivery_points",
                          "Alias": "dp",
                          "Startup Cost": 0.0,
                          "Total Cost": 1.09,
                          "Plan Rows": 8,
                          "Plan Width": 31,
                          "Actual Startup Time": 0.011,
                          "Actual Total Time": 0.013,
                          "Actual Rows": 8,
                          "Actual Loops": 1,
                          "Filter": "is_active",
                          "Rows Removed by Filter": 1,
                          "Shared Hit Blocks": 1,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0
                        }
                      ]
                    }
                  ]
                },
                {
                  "Node Type": "Materialize",
                  "Parent Relationship": "Inner",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Startup Cost": 0.0,
                  "Total Cost": 1.14,
                  "Plan Rows": 6,
                  "Plan Width": 40,
                  "Actual Startup Time": 0.0,
                  "Actual Total Time": 0.0,
                  "Actual Rows": 2,
                  "Actual Loops": 10026,
                  "Shared Hit Blocks": 1,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0,
                  "Plans": [
                    {
                      "Node Type": "Seq Scan",
                      "Parent Relationship": "Outer",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Relation Name": "products",
                      "Alias": "p",
                      "Startup Cost": 0.0,
                      "Total Cost": 1.1,
                      "Plan Rows": 6,
                      "Plan Width": 40,
                      "Actual Startup Time": 0.01,
                      "Actual Total Time": 0.013,
                      "Actual Rows": 6,
                      "Actual Loops": 1,
                      "Filter": "(is_active AND ((fuel_type)::text = ANY ('{Methanol,Ethanol,FAME,LNG}'::text[])))",
                      "Rows Removed by Filter": 1,
                      "Shared Hit Blocks": 1,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0
                    }
                  ]
                },
                {
                  "Node Type": "CTE Scan",
                  "Parent Relationship": "SubPlan",
                  "Subplan Name": "SubPlan 20",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "CTE Name": "policy_products",
                  "Alias": "pd",
                  "Startup Cost": 0.0,
                  "Total Cost": 2.25,
                  "Plan Rows": 2,
                  "Plan Width": 112,
                  "Actual Startup Time": 0.0,
                  "Actual Total Time": 0.0,
                  "Actual Rows": 0,
                  "Actual Loops": 0,
                  "Filter": "(qualification_profile = ANY ('{fame_v1,lng_v1}'::text[]))",
                  "Rows Removed by Filter": 0,
                  "Shared Hit Blocks": 0,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0
                },
                {
                  "Node Type": "CTE Scan",
                  "Parent Relationship": "SubPlan",
                  "Subplan Name": "SubPlan 22",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "CTE Name": "policy_products",
                  "Alias": "po",
                  "Startup Cost": 0.0,
                  "Total Cost": 2.25,
                  "Plan Rows": 1,
                  "Plan Width": 112,
                  "Actual Startup Time": 0.0,
                  "Actual Total Time": 0.0,
                  "Actual Rows": 0,
                  "Actual Loops": 0,
                  "Filter": "(qualification_profile = 'fame_v1'::text)",
                  "Rows Removed by Filter": 0,
                  "Shared Hit Blocks": 0,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0
                }
              ]
            },
            {
              "Node Type": "CTE Scan",
              "Parent Relationship": "Inner",
              "Parallel Aware": false,
              "Async Capable": false,
              "CTE Name": "policy_products",
              "Alias": "pp",
              "Startup Cost": 0.0,
              "Total Cost": 2.25,
              "Plan Rows": 1,
              "Plan Width": 144,
              "Actual Startup Time": 0.0,
              "Actual Total Time": 0.001,
              "Actual Rows": 3,
              "Actual Loops": 10025,
              "Filter": "(execution_mode = 'ORDERBOOK'::text)",
              "Rows Removed by Filter": 0,
              "Shared Hit Blocks": 0,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0
            },
            {
              "Node Type": "Function Scan",
              "Parent Relationship": "SubPlan",
              "Subplan Name": "SubPlan 23",
              "Parallel Aware": false,
              "Async Capable": false,
              "Function Name": "jsonb_array_elements_text",
              "Alias": "lane_2",
              "Startup Cost": 0.0,
              "Total Cost": 1.75,
              "Plan Rows": 1,
              "Plan Width": 0,
              "Actual Startup Time": 0.004,
              "Actual Total Time": 0.004,
              "Actual Rows": 1,
              "Actual Loops": 10024,
              "Filter": "(value = (o.delivery_point_id)::text)",
              "Rows Removed by Filter": 3,
              "Shared Hit Blocks": 0,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0
            }
          ]
        },
        {
          "Node Type": "Function Scan",
          "Parent Relationship": "Inner",
          "Parallel Aware": false,
          "Async Capable": false,
          "Function Name": "jsonb_to_recordset",
          "Alias": "policy_point",
          "Startup Cost": 0.0,
          "Total Cost": 1.0,
          "Plan Rows": 100,
          "Plan Width": 80,
          "Actual Startup Time": 0.0,
          "Actual Total Time": 0.0,
          "Actual Rows": 4,
          "Actual Loops": 10023,
          "Shared Hit Blocks": 0,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0
        }
      ]
    }
  ]
}
```

### rust /api/orderbook/bids rows=20

```sql
WITH eligible_orders AS (-- Shared public-order visibility projection (Task 6).
--
-- This is the complete bound internal relation: every caller value is a parameter, so
-- no route ever concatenates text into SQL. Parameters are
--   $1  attested market policy (jsonb text)
--   $2  captured UTC instant (timestamptz)
--   $3  Singapore date (text, YYYY-MM-DD)
--   $4  include_off_spec (boolean)
--   $5  product_id (uuid, null = any)
--   $6  delivery_point_id (uuid, null = any)
--   $7  side (text, null = any)
--   $8  fuel_type (text, null = any)
--   $9  market_product code (text, null = any)
--   $10 region (text, null = any)
--   $11 availability_window (text, null = any)
--   $12 surface: orderbook, listings, or candidates (text)
--   $13 runtime KYC execution gate enabled (boolean)
-- No pagination here: consumers rank, aggregate and page the shared relation.
--
-- Product identity comes only from the policy. The statement never names a
-- product, a product code, or a product UUID.
WITH policy_products AS (
    SELECT *
    FROM jsonb_to_recordset(($1::jsonb) -> 'products') AS policy_product(
        id uuid,
        code text,
        name text,
        fuel_type text,
        fuel_grade text,
        execution_mode text,
        lanes jsonb,
        qualification_profile text,
        display_order integer
    )
),
policy_points AS (
    SELECT *
    FROM jsonb_to_recordset(($1::jsonb) -> 'points') AS policy_point(
        id uuid,
        name text,
        region text,
        timezone text,
        display_order integer
    )
)
SELECT o.*, p.name AS product_name, p.fuel_type AS product_fuel_type,
       p.fuel_grade AS product_fuel_grade, p.is_active AS product_is_active,
       dp.name AS delivery_point_name, dp.region AS delivery_point_region,
       (SELECT pp.code FROM policy_products pp WHERE pp.id = p.id) AS market_product,
       -- Internal only. Never serialize this object as a public DTO.
       to_jsonb(o) || jsonb_build_object(
           'quantity_mt', o.quantity_mt::text,
           'remaining_quantity_mt', o.remaining_quantity_mt::text,
           'price_per_mt_usd', o.price_per_mt_usd::text,
           'carbon_intensity_gco2_mj', o.carbon_intensity_gco2_mj::text,
           'energy_density_mj_kg', o.energy_density_mj_kg::text
       ) AS private_digest_input
FROM orderbook_orders AS o
JOIN products AS p ON p.id = o.product_id
JOIN delivery_points AS dp ON dp.id = o.delivery_point_id
WHERE o.status IN ('OPEN', 'PARTIALLY_FILLED')
    -- Strict canonical product identity, active row, and approved lane.
    AND p.is_active
    AND EXISTS (
        SELECT 1
        FROM policy_products AS pp
        WHERE pp.execution_mode = 'ORDERBOOK'
          AND pp.id = p.id
          AND pp.name = p.name
          AND pp.fuel_type = p.fuel_type
          AND pp.fuel_grade = p.fuel_grade
          AND EXISTS (
              SELECT 1
              FROM jsonb_array_elements_text(pp.lanes) AS lane(value)
              WHERE lane.value = o.delivery_point_id::text
          )
    )
    -- Strict canonical delivery-point identity and active row.
    AND dp.is_active
    AND EXISTS (
        SELECT 1
        FROM policy_points AS pq
        WHERE pq.id = dp.id AND pq.name = dp.name AND pq.region = dp.region
    )
    -- Canonical availability-window grammar.
    AND ($12::text <> 'orderbook' OR (
        o.availability_window = 'SPOT'
        OR o.availability_window ~ '^[0-9]{4}-(0[1-9]|1[0-2])$'
        OR o.availability_window ~ '^[0-9]{4}-Q[1-4]$'
        OR o.availability_window ~ '^[0-9]{4}-CAL$'
    ))
    AND (-- Shared expiry, owner admission and shallow declaration rule.
-- Requires policy_products and alias o. Binds $2, $3 and $13.
TRUE
    -- Currency: an explicit future expiry, or a null expiry on REAL/UNKNOWN.
    AND (
        o.expires_at > $2::timestamptz
        OR (
            o.expires_at IS NULL
            AND o.provenance IN ('REAL', 'UNKNOWN')
        )
    )
    -- Owner admission. Assisted rows use the narrower administrator rule.
    AND (
        CASE
            WHEN o.creation_method = 'MARKET_SUPPORT' THEN EXISTS (
                SELECT 1
                FROM users AS actor
                WHERE actor.id = o.owner_user_id
                  AND actor.id = o.created_by_actor_user_id
                  AND actor.role = 'ADMIN'
                  AND actor.status = 'APPROVED'
                  AND EXISTS (
                      SELECT 1
                      FROM users AS member
                      WHERE member.organization_id = o.organization_id
                        AND member.role IN ('BUYER', 'SUPPLIER')
                        AND member.status = 'APPROVED'
                        AND member.email_verified
                        AND NOT member.must_change_password
                        AND member.kyc_status <> 'REJECTED'
                        AND CASE WHEN $13::boolean THEN (
                            (member.kyc_status = 'APPROVED'
                             AND member.kyc_organization_id = member.organization_id)
                            OR EXISTS (
                                SELECT 1 FROM kyc_execution_grace AS grace
                                WHERE grace.user_id = member.id
                                  AND grace.organization_id = member.organization_id
                                  AND grace.expires_at > $2::timestamptz
                            )
                        ) ELSE (
                            member.kyc_organization_id IS NULL
                            OR member.kyc_organization_id = member.organization_id
                        ) END
                  )
                  AND EXISTS (
                      SELECT 1
                      FROM organizations AS assisted_org
                      WHERE assisted_org.id = o.organization_id
                        AND assisted_org.verification_status = 'APPROVED'
                        AND assisted_org.provenance = 'REAL'
                  )
            )
            ELSE (
                o.owner_user_id IS NULL
                OR EXISTS (
                    SELECT 1
                    FROM users AS owner
                    WHERE owner.id = o.owner_user_id
                      AND owner.organization_id = o.organization_id
                      AND owner.role IN ('BUYER', 'SUPPLIER')
                      AND owner.status = 'APPROVED'
                      AND owner.email_verified
                      AND NOT owner.must_change_password
                      AND owner.kyc_status <> 'REJECTED'
                      AND CASE WHEN $13::boolean THEN (
                          (owner.kyc_status = 'APPROVED'
                           AND owner.kyc_organization_id = owner.organization_id)
                          OR EXISTS (
                              SELECT 1 FROM kyc_execution_grace AS grace
                              WHERE grace.user_id = owner.id
                                AND grace.organization_id = owner.organization_id
                                AND grace.expires_at > $2::timestamptz
                          )
                      ) ELSE (
                          owner.kyc_organization_id IS NULL
                          OR owner.kyc_organization_id = owner.organization_id
                      ) END
                      AND EXISTS (
                          SELECT 1
                          FROM organizations AS org
                          WHERE org.id = o.organization_id
                            AND org.verification_status = 'APPROVED'
                      )
                )
            )
        END
    )
    -- fame_v1 declaration currency, keyed by policy profile.
    AND (
        NOT EXISTS (
            SELECT 1
            FROM policy_products AS pf
            WHERE pf.qualification_profile = 'fame_v1'
              AND pf.id = o.product_id
        )
        OR (
            EXISTS (
                SELECT 1
                FROM policy_products AS pf
                WHERE pf.qualification_profile = 'fame_v1'
                  AND pf.id = o.product_id
                  AND EXISTS (
                      SELECT 1
                      FROM jsonb_array_elements_text(pf.lanes) AS lane(value)
                      WHERE lane.value = o.delivery_point_id::text
                  )
            )
            AND o.fame_terms ->> 'side' = o.side
            AND o.off_spec = false
            AND (
                o.side <> 'ASK'
                OR (
                    o.fame_terms ->> 'certificate_valid_until' >= $3::text
                    AND o.msds_available
                    AND o.certification_declared
                    AND length(btrim(coalesce(o.certification_scheme, ''))) > 0
                )
            )
        )
    )
    -- lng_v1 declaration currency, keyed by policy profile.
    AND (
        NOT EXISTS (
            SELECT 1
            FROM policy_products AS pl
            WHERE pl.qualification_profile = 'lng_v1'
              AND pl.id = o.product_id
        )
        OR (
            EXISTS (
                SELECT 1
                FROM policy_products AS pl
                WHERE pl.qualification_profile = 'lng_v1'
                  AND pl.id = o.product_id
                  AND EXISTS (
                      SELECT 1
                      FROM jsonb_array_elements_text(pl.lanes) AS lane(value)
                      WHERE lane.value = o.delivery_point_id::text
                  )
            )
            AND o.lng_terms ->> 'side' = o.side
            AND o.off_spec = false
        )
    )
)
    -- Independently labelled public rows.
    AND o.provenance IN ('REAL', 'DEMO')
    -- Admitted fuel types.
    AND p.fuel_type IN ('Methanol', 'Ethanol', 'FAME', 'LNG')
    -- Scalar ASK quality declarations.
    AND ($12::text <> 'orderbook' OR (
        o.side <> 'ASK'
        OR (
            length(btrim(coalesce(o.certification_scheme, ''))) > 0
            AND o.certification_declared
            AND length(btrim(coalesce(o.specification_standard, ''))) > 0
            AND o.msds_available
            AND (
                EXISTS (
                    SELECT 1
                    FROM policy_products AS pd
                    WHERE pd.qualification_profile IN ('fame_v1', 'lng_v1')
                      AND pd.id = p.id
                      AND pd.name = p.name
                      AND pd.fuel_type = p.fuel_type
                      AND pd.fuel_grade = p.fuel_grade
                )
                OR o.carbon_intensity_gco2_mj IS NOT NULL
            )
            AND length(btrim(coalesce(o.feedstock, ''))) > 0
            AND (
                EXISTS (
                    SELECT 1
                    FROM policy_products AS po
                    WHERE po.qualification_profile = 'fame_v1'
                      AND po.id = p.id
                      AND po.name = p.name
                      AND po.fuel_type = p.fuel_type
                      AND po.fuel_grade = p.fuel_grade
                )
                OR length(btrim(coalesce(o.origin, ''))) > 0
            )
        )
    )
    )
    -- Off-spec rows are hidden unless the caller asks for them.
    AND ($12::text <> 'orderbook' OR o.off_spec = false OR $4::boolean)
    AND ($12::text <> 'listings' OR o.side = 'ASK')
    AND $12::text IN ('orderbook', 'listings', 'candidates')
    -- Route filters.
    AND ($5::uuid IS NULL OR o.product_id = $5::uuid)
    AND ($6::uuid IS NULL OR o.delivery_point_id = $6::uuid)
    AND ($7::text IS NULL OR o.side = $7::text)
    AND ($8::text IS NULL OR p.fuel_type = $8::text)
    AND (
        $9::text IS NULL
        OR EXISTS (
            SELECT 1
            FROM policy_products AS pm
            WHERE pm.code = $9::text
              AND pm.id = p.id
              AND pm.name = p.name
              AND pm.fuel_type = p.fuel_type
              AND pm.fuel_grade = p.fuel_grade
        )
    )
    AND ($10::text IS NULL OR $10::text = '' OR dp.region = $10::text OR dp.name = $10::text)
    AND ($11::text IS NULL OR o.availability_window = $11::text)
) SELECT page.*, COALESCE(org.supplier_tier, 'INDEPENDENT') AS tier_label
FROM (
SELECT o.*
FROM eligible_orders o
ORDER BY
    CASE WHEN $14 = 'price_asc' THEN o.price_per_mt_usd END ASC,
    CASE WHEN $14 = 'price_desc' THEN o.price_per_mt_usd END DESC,
    CASE WHEN $14 IN ('price_asc', 'price_desc') THEN o.acceptance_ordinal END ASC,
    CASE WHEN $14 IN ('price_asc', 'price_desc') THEN o.id END ASC,
    CASE WHEN $14 = 'quantity_desc' THEN o.remaining_quantity_mt END DESC,
    CASE WHEN $14 IN ('newest', 'quantity_desc') THEN o.created_at END DESC,
    CASE WHEN $14 IN ('newest', 'quantity_desc') THEN o.id END DESC
LIMIT $15 OFFSET $16
) AS page
LEFT JOIN organizations org ON org.id = page.organization_id

ORDER BY
    CASE WHEN $14 = 'price_asc' THEN page.price_per_mt_usd END ASC,
    CASE WHEN $14 = 'price_desc' THEN page.price_per_mt_usd END DESC,
    CASE WHEN $14 IN ('price_asc', 'price_desc') THEN page.acceptance_ordinal END ASC,
    CASE WHEN $14 IN ('price_asc', 'price_desc') THEN page.id END ASC,
    CASE WHEN $14 = 'quantity_desc' THEN page.remaining_quantity_mt END DESC,
    CASE WHEN $14 IN ('newest', 'quantity_desc') THEN page.created_at END DESC,
    CASE WHEN $14 IN ('newest', 'quantity_desc') THEN page.id END DESC

```

```json
{
  "Node Type": "Nested Loop",
  "Parallel Aware": false,
  "Async Capable": false,
  "Join Type": "Left",
  "Startup Cost": 3937181.74,
  "Total Cost": 3937183.05,
  "Plan Rows": 1,
  "Plan Width": 3163,
  "Actual Startup Time": 1772.349,
  "Actual Total Time": 1772.503,
  "Actual Rows": 20,
  "Actual Loops": 1,
  "Inner Unique": true,
  "Join Filter": "(org.id = o.organization_id)",
  "Rows Removed by Join Filter": 60,
  "Shared Hit Blocks": 30418,
  "Shared Read Blocks": 0,
  "Shared Dirtied Blocks": 0,
  "Shared Written Blocks": 0,
  "Local Hit Blocks": 0,
  "Local Read Blocks": 0,
  "Local Dirtied Blocks": 0,
  "Local Written Blocks": 0,
  "Temp Read Blocks": 0,
  "Temp Written Blocks": 0,
  "Plans": [
    {
      "Node Type": "Limit",
      "Parent Relationship": "Outer",
      "Parallel Aware": false,
      "Async Capable": false,
      "Startup Cost": 3937181.74,
      "Total Cost": 3937181.75,
      "Plan Rows": 1,
      "Plan Width": 3131,
      "Actual Startup Time": 1772.332,
      "Actual Total Time": 1772.356,
      "Actual Rows": 20,
      "Actual Loops": 1,
      "Shared Hit Blocks": 30398,
      "Shared Read Blocks": 0,
      "Shared Dirtied Blocks": 0,
      "Shared Written Blocks": 0,
      "Local Hit Blocks": 0,
      "Local Read Blocks": 0,
      "Local Dirtied Blocks": 0,
      "Local Written Blocks": 0,
      "Temp Read Blocks": 0,
      "Temp Written Blocks": 0,
      "Plans": [
        {
          "Node Type": "Sort",
          "Parent Relationship": "Outer",
          "Parallel Aware": false,
          "Async Capable": false,
          "Startup Cost": 3937181.74,
          "Total Cost": 3937181.75,
          "Plan Rows": 1,
          "Plan Width": 3131,
          "Actual Startup Time": 1772.328,
          "Actual Total Time": 1772.348,
          "Actual Rows": 20,
          "Actual Loops": 1,
          "Sort Key": [
            "o.created_at DESC",
            "o.id DESC"
          ],
          "Sort Method": "top-N heapsort",
          "Sort Space Used": 99,
          "Sort Space Type": "Memory",
          "Shared Hit Blocks": 30398,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0,
          "Plans": [
            {
              "Node Type": "Subquery Scan",
              "Parent Relationship": "Outer",
              "Parallel Aware": false,
              "Async Capable": false,
              "Alias": "o",
              "Startup Cost": 1.0,
              "Total Cost": 3937181.73,
              "Plan Rows": 1,
              "Plan Width": 3131,
              "Actual Startup Time": 0.72,
              "Actual Total Time": 1741.736,
              "Actual Rows": 10023,
              "Actual Loops": 1,
              "Shared Hit Blocks": 30398,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0,
              "Plans": [
                {
                  "Node Type": "Nested Loop",
                  "Parent Relationship": "Subquery",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Join Type": "Semi",
                  "Startup Cost": 1.0,
                  "Total Cost": 3937181.73,
                  "Plan Rows": 1,
                  "Plan Width": 2987,
                  "Actual Startup Time": 0.715,
                  "Actual Total Time": 1732.701,
                  "Actual Rows": 10023,
                  "Actual Loops": 1,
                  "Inner Unique": false,
                  "Join Filter": "((policy_point.id = dp.id) AND ((dp.name)::text = policy_point.name) AND (policy_point.region = (dp.region)::text))",
                  "Rows Removed by Join Filter": 30069,
                  "Shared Hit Blocks": 30398,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0,
                  "Plans": [
                    {
                      "Node Type": "Function Scan",
                      "Parent Relationship": "InitPlan",
                      "Subplan Name": "CTE policy_products",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Function Name": "jsonb_to_recordset",
                      "Alias": "policy_product",
                      "Startup Cost": 0.0,
                      "Total Cost": 1.0,
                      "Plan Rows": 100,
                      "Plan Width": 244,
                      "Actual Startup Time": 0.089,
                      "Actual Total Time": 0.091,
                      "Actual Rows": 6,
                      "Actual Loops": 1,
                      "Shared Hit Blocks": 0,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0
                    },
                    {
                      "Node Type": "Nested Loop",
                      "Parent Relationship": "Outer",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Join Type": "Semi",
                      "Startup Cost": 0.0,
                      "Total Cost": 3937175.69,
                      "Plan Rows": 1,
                      "Plan Width": 5862,
                      "Actual Startup Time": 0.492,
                      "Actual Total Time": 437.398,
                      "Actual Rows": 10023,
                      "Actual Loops": 1,
                      "Inner Unique": false,
                      "Join Filter": "((pp.id = p.id) AND ((p.name)::text = pp.name) AND (pp.fuel_type = (p.fuel_type)::text) AND (pp.fuel_grade = (p.fuel_grade)::text) AND EXISTS(SubPlan 24))",
                      "Rows Removed by Join Filter": 20058,
                      "Shared Hit Blocks": 30398,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0,
                      "Plans": [
                        {
                          "Node Type": "Nested Loop",
                          "Parent Relationship": "Outer",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Join Type": "Inner",
                          "Startup Cost": 0.0,
                          "Total Cost": 3935472.95,
                          "Plan Rows": 954,
                          "Plan Width": 5862,
                          "Actual Startup Time": 0.441,
                          "Actual Total Time": 321.554,
                          "Actual Rows": 10025,
                          "Actual Loops": 1,
                          "Inner Unique": true,
                          "Join Filter": "((o_1.product_id = p.id) AND (((o_1.side)::text <> 'ASK'::text) OR ((length(btrim((COALESCE(o_1.certification_scheme, ''::character varying))::text)) > 0) AND o_1.certification_declared AND (length(btrim((COALESCE(o_1.specification_standard, ''::character varying))::text)) > 0) AND o_1.msds_available AND ((ANY ((p.id = (hashed SubPlan 21).col1) AND ((p.name)::text = (hashed SubPlan 21).col2) AND ((p.fuel_type)::text = (hashed SubPlan 21).col3) AND ((p.fuel_grade)::text = (hashed SubPlan 21).col4))) OR (o_1.carbon_intensity_gco2_mj IS NOT NULL)) AND (length(btrim((COALESCE(o_1.feedstock, ''::character varying))::text)) > 0) AND ((ANY ((p.id = (hashed SubPlan 23).col1) AND ((p.name)::text = (hashed SubPlan 23).col2) AND ((p.fuel_type)::text = (hashed SubPlan 23).col3) AND ((p.fuel_grade)::text = (hashed SubPlan 23).col4))) OR (length(btrim((COALESCE(o_1.origin, ''::character varying))::text)) > 0)))))",
                          "Rows Removed by Join Filter": 10035,
                          "Shared Hit Blocks": 30398,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0,
                          "Plans": [
                            {
                              "Node Type": "Nested Loop",
                              "Parent Relationship": "Outer",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Join Type": "Inner",
                              "Startup Cost": 0.0,
                              "Total Cost": 3896091.42,
                              "Plan Rows": 1116,
                              "Plan Width": 5821,
                              "Actual Startup Time": 0.416,
                              "Actual Total Time": 305.66,
                              "Actual Rows": 10026,
                              "Actual Loops": 1,
                              "Inner Unique": true,
                              "Join Filter": "(o_1.delivery_point_id = dp.id)",
                              "Rows Removed by Join Filter": 15,
                              "Shared Hit Blocks": 30397,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0,
                              "Plans": [
                                {
                                  "Node Type": "Seq Scan",
                                  "Parent Relationship": "Outer",
                                  "Parallel Aware": false,
                                  "Async Capable": false,
                                  "Relation Name": "orderbook_orders",
                                  "Alias": "o_1",
                                  "Startup Cost": 0.0,
                                  "Total Cost": 3895952.69,
                                  "Plan Rows": 1255,
                                  "Plan Width": 5790,
                                  "Actual Startup Time": 0.373,
                                  "Actual Total Time": 291.577,
                                  "Actual Rows": 10027,
                                  "Actual Loops": 1,
                                  "Filter": "((NOT off_spec) AND ((status)::text = ANY ('{OPEN,PARTIALLY_FILLED}'::text[])) AND ((provenance)::text = ANY ('{REAL,DEMO}'::text[])) AND ((side)::text = 'BID'::text) AND ((expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone) OR ((expires_at IS NULL) AND ((provenance)::text = ANY ('{REAL,UNKNOWN}'::text[])))) AND (((availability_window)::text = 'SPOT'::text) OR ((availability_window)::text ~ '^[0-9]{4}-(0[1-9]|1[0-2])$'::text) OR ((availability_window)::text ~ '^[0-9]{4}-Q[1-4]$'::text) OR ((availability_window)::text ~ '^[0-9]{4}-CAL$'::text)) AND CASE WHEN ((creation_method)::text = 'MARKET_SUPPORT'::text) THEN EXISTS(SubPlan 7) ELSE ((owner_user_id IS NULL) OR EXISTS(SubPlan 11)) END AND ((NOT (ANY (product_id = (hashed SubPlan 17).col1))) OR (EXISTS(SubPlan 19) AND ((lng_terms ->> 'side'::text) = (side)::text) AND (NOT off_spec))) AND ((NOT (ANY (product_id = (hashed SubPlan 13).col1))) OR (EXISTS(SubPlan 15) AND ((fame_terms ->> 'side'::text) = (side)::text) AND (NOT off_spec) AND (((side)::text <> 'ASK'::text) OR (((fame_terms ->> 'certificate_valid_until'::text) >= '2026-10-15'::text) AND msds_available AND certification_declared AND (length(btrim((COALESCE(certification_scheme, ''::character varying))::text)) > 0))))))",
                                  "Rows Removed by Filter": 52,
                                  "Shared Hit Blocks": 30396,
                                  "Shared Read Blocks": 0,
                                  "Shared Dirtied Blocks": 0,
                                  "Shared Written Blocks": 0,
                                  "Local Hit Blocks": 0,
                                  "Local Read Blocks": 0,
                                  "Local Dirtied Blocks": 0,
                                  "Local Written Blocks": 0,
                                  "Temp Read Blocks": 0,
                                  "Temp Written Blocks": 0,
                                  "Plans": [
                                    {
                                      "Node Type": "Result",
                                      "Parent Relationship": "SubPlan",
                                      "Subplan Name": "SubPlan 7",
                                      "Parallel Aware": false,
                                      "Async Capable": false,
                                      "Startup Cost": 14.05,
                                      "Total Cost": 15.45,
                                      "Plan Rows": 1,
                                      "Plan Width": 0,
                                      "Actual Startup Time": 0.023,
                                      "Actual Total Time": 0.024,
                                      "Actual Rows": 0,
                                      "Actual Loops": 7,
                                      "One-Time Filter": "((InitPlan 5).col1 AND (InitPlan 6).col1 AND (o_1.owner_user_id = o_1.created_by_actor_user_id))",
                                      "Shared Hit Blocks": 22,
                                      "Shared Read Blocks": 0,
                                      "Shared Dirtied Blocks": 0,
                                      "Shared Written Blocks": 0,
                                      "Local Hit Blocks": 0,
                                      "Local Read Blocks": 0,
                                      "Local Dirtied Blocks": 0,
                                      "Local Written Blocks": 0,
                                      "Temp Read Blocks": 0,
                                      "Temp Written Blocks": 0,
                                      "Plans": [
                                        {
                                          "Node Type": "Index Scan",
                                          "Parent Relationship": "InitPlan",
                                          "Subplan Name": "InitPlan 5",
                                          "Parallel Aware": false,
                                          "Async Capable": false,
                                          "Scan Direction": "Forward",
                                          "Index Name": "ix_users_organization_id_id",
                                          "Relation Name": "users",
                                          "Alias": "member",
                                          "Startup Cost": 0.14,
                                          "Total Cost": 12.82,
                                          "Plan Rows": 1,
                                          "Plan Width": 0,
                                          "Actual Startup Time": 0.01,
                                          "Actual Total Time": 0.011,
                                          "Actual Rows": 1,
                                          "Actual Loops": 7,
                                          "Index Cond": "(organization_id = o_1.organization_id)",
                                          "Rows Removed by Index Recheck": 0,
                                          "Filter": "(email_verified AND (NOT must_change_password) AND ((role)::text = ANY ('{BUYER,SUPPLIER}'::text[])) AND ((kyc_status)::text <> 'REJECTED'::text) AND ((status)::text = 'APPROVED'::text) AND ((((kyc_status)::text = 'APPROVED'::text) AND (kyc_organization_id = organization_id)) OR (ANY ((id = (hashed SubPlan 4).col1) AND (organization_id = (hashed SubPlan 4).col2)))))",
                                          "Rows Removed by Filter": 0,
                                          "Shared Hit Blocks": 15,
                                          "Shared Read Blocks": 0,
                                          "Shared Dirtied Blocks": 0,
                                          "Shared Written Blocks": 0,
                                          "Local Hit Blocks": 0,
                                          "Local Read Blocks": 0,
                                          "Local Dirtied Blocks": 0,
                                          "Local Written Blocks": 0,
                                          "Temp Read Blocks": 0,
                                          "Temp Written Blocks": 0,
                                          "Plans": [
                                            {
                                              "Node Type": "Seq Scan",
                                              "Parent Relationship": "SubPlan",
                                              "Subplan Name": "SubPlan 4",
                                              "Parallel Aware": false,
                                              "Async Capable": false,
                                              "Relation Name": "kyc_execution_grace",
                                              "Alias": "grace",
                                              "Startup Cost": 0.0,
                                              "Total Cost": 1.1,
                                              "Plan Rows": 6,
                                              "Plan Width": 32,
                                              "Actual Startup Time": 0.005,
                                              "Actual Total Time": 0.006,
                                              "Actual Rows": 6,
                                              "Actual Loops": 1,
                                              "Filter": "(expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone)",
                                              "Rows Removed by Filter": 2,
                                              "Shared Hit Blocks": 1,
                                              "Shared Read Blocks": 0,
                                              "Shared Dirtied Blocks": 0,
                                              "Shared Written Blocks": 0,
                                              "Local Hit Blocks": 0,
                                              "Local Read Blocks": 0,
                                              "Local Dirtied Blocks": 0,
                                              "Local Written Blocks": 0,
                                              "Temp Read Blocks": 0,
                                              "Temp Written Blocks": 0
                                            }
                                          ]
                                        },
                                        {
                                          "Node Type": "Seq Scan",
                                          "Parent Relationship": "InitPlan",
                                          "Subplan Name": "InitPlan 6",
                                          "Parallel Aware": false,
                                          "Async Capable": false,
                                          "Relation Name": "organizations",
                                          "Alias": "assisted_org",
                                          "Startup Cost": 0.0,
                                          "Total Cost": 1.23,
                                          "Plan Rows": 1,
                                          "Plan Width": 0,
                                          "Actual Startup Time": 0.015,
                                          "Actual Total Time": 0.015,
                                          "Actual Rows": 1,
                                          "Actual Loops": 4,
                                          "Filter": "((id = o_1.organization_id) AND ((verification_status)::text = 'APPROVED'::text) AND ((provenance)::text = 'REAL'::text))",
                                          "Rows Removed by Filter": 4,
                                          "Shared Hit Blocks": 4,
                                          "Shared Read Blocks": 0,
                                          "Shared Dirtied Blocks": 0,
                                          "Shared Written Blocks": 0,
                                          "Local Hit Blocks": 0,
                                          "Local Read Blocks": 0,
                                          "Local Dirtied Blocks": 0,
                                          "Local Written Blocks": 0,
                                          "Temp Read Blocks": 0,
                                          "Temp Written Blocks": 0
                                        },
                                        {
                                          "Node Type": "Seq Scan",
                                          "Parent Relationship": "Outer",
                                          "Parallel Aware": false,
                                          "Async Capable": false,
                                          "Relation Name": "users",
                                          "Alias": "actor",
                                          "Startup Cost": 14.05,
                                          "Total Cost": 15.45,
                                          "Plan Rows": 1,
                                          "Plan Width": 0,
                                          "Actual Startup Time": 0.005,
                                          "Actual Total Time": 0.005,
                                          "Actual Rows": 1,
                                          "Actual Loops": 3,
                                          "Filter": "((id = o_1.created_by_actor_user_id) AND ((role)::text = 'ADMIN'::text) AND ((status)::text = 'APPROVED'::text))",
                                          "Rows Removed by Filter": 13,
                                          "Shared Hit Blocks": 3,
                                          "Shared Read Blocks": 0,
                                          "Shared Dirtied Blocks": 0,
                                          "Shared Written Blocks": 0,
                                          "Local Hit Blocks": 0,
                                          "Local Read Blocks": 0,
                                          "Local Dirtied Blocks": 0,
                                          "Local Written Blocks": 0,
                                          "Temp Read Blocks": 0,
                                          "Temp Written Blocks": 0
                                        }
                                      ]
                                    },
                                    {
                                      "Node Type": "Result",
                                      "Parent Relationship": "SubPlan",
                                      "Subplan Name": "SubPlan 11",
                                      "Parallel Aware": false,
                                      "Async Capable": false,
                                      "Startup Cost": 1.33,
                                      "Total Cost": 10.5,
                                      "Plan Rows": 1,
                                      "Plan Width": 0,
                                      "Actual Startup Time": 0.016,
                                      "Actual Total Time": 0.016,
                                      "Actual Rows": 1,
                                      "Actual Loops": 10036,
                                      "One-Time Filter": "(InitPlan 10).col1",
                                      "Shared Hit Blocks": 30106,
                                      "Shared Read Blocks": 0,
                                      "Shared Dirtied Blocks": 0,
                                      "Shared Written Blocks": 0,
                                      "Local Hit Blocks": 0,
                                      "Local Read Blocks": 0,
                                      "Local Dirtied Blocks": 0,
                                      "Local Written Blocks": 0,
                                      "Temp Read Blocks": 0,
                                      "Temp Written Blocks": 0,
                                      "Plans": [
                                        {
                                          "Node Type": "Seq Scan",
                                          "Parent Relationship": "InitPlan",
                                          "Subplan Name": "InitPlan 10",
                                          "Parallel Aware": false,
                                          "Async Capable": false,
                                          "Relation Name": "organizations",
                                          "Alias": "org_1",
                                          "Startup Cost": 0.0,
                                          "Total Cost": 1.2,
                                          "Plan Rows": 1,
                                          "Plan Width": 0,
                                          "Actual Startup Time": 0.003,
                                          "Actual Total Time": 0.003,
                                          "Actual Rows": 1,
                                          "Actual Loops": 10036,
                                          "Filter": "((id = o_1.organization_id) AND ((verification_status)::text = 'APPROVED'::text))",
                                          "Rows Removed by Filter": 3,
                                          "Shared Hit Blocks": 10036,
                                          "Shared Read Blocks": 0,
                                          "Shared Dirtied Blocks": 0,
                                          "Shared Written Blocks": 0,
                                          "Local Hit Blocks": 0,
                                          "Local Read Blocks": 0,
                                          "Local Dirtied Blocks": 0,
                                          "Local Written Blocks": 0,
                                          "Temp Read Blocks": 0,
                                          "Temp Written Blocks": 0
                                        },
                                        {
                                          "Node Type": "Index Scan",
                                          "Parent Relationship": "Outer",
                                          "Parallel Aware": false,
                                          "Async Capable": false,
                                          "Scan Direction": "Forward",
                                          "Index Name": "ix_users_organization_id_id",
                                          "Relation Name": "users",
                                          "Alias": "owner",
                                          "Startup Cost": 1.33,
                                          "Total Cost": 10.5,
                                          "Plan Rows": 1,
                                          "Plan Width": 0,
                                          "Actual Startup Time": 0.008,
                                          "Actual Total Time": 0.008,
                                          "Actual Rows": 1,
                                          "Actual Loops": 10035,
                                          "Index Cond": "((organization_id = o_1.organization_id) AND (id = o_1.owner_user_id))",
                                          "Rows Removed by Index Recheck": 0,
                                          "Filter": "(email_verified AND (NOT must_change_password) AND ((role)::text = ANY ('{BUYER,SUPPLIER}'::text[])) AND ((kyc_status)::text <> 'REJECTED'::text) AND ((status)::text = 'APPROVED'::text) AND ((((kyc_status)::text = 'APPROVED'::text) AND (kyc_organization_id = organization_id)) OR (ANY ((id = (hashed SubPlan 9).col1) AND (organization_id = (hashed SubPlan 9).col2)))))",
                                          "Rows Removed by Filter": 0,
                                          "Shared Hit Blocks": 20070,
                                          "Shared Read Blocks": 0,
                                          "Shared Dirtied Blocks": 0,
                                          "Shared Written Blocks": 0,
                                          "Local Hit Blocks": 0,
                                          "Local Read Blocks": 0,
                                          "Local Dirtied Blocks": 0,
                                          "Local Written Blocks": 0,
                                          "Temp Read Blocks": 0,
                                          "Temp Written Blocks": 0,
                                          "Plans": [
                                            {
                                              "Node Type": "Seq Scan",
                                              "Parent Relationship": "SubPlan",
                                              "Subplan Name": "SubPlan 9",
                                              "Parallel Aware": false,
                                              "Async Capable": false,
                                              "Relation Name": "kyc_execution_grace",
                                              "Alias": "grace_1",
                                              "Startup Cost": 0.0,
                                              "Total Cost": 1.1,
                                              "Plan Rows": 6,
                                              "Plan Width": 32,
                                              "Actual Startup Time": 0.025,
                                              "Actual Total Time": 0.027,
                                              "Actual Rows": 6,
                                              "Actual Loops": 1,
                                              "Filter": "(expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone)",
                                              "Rows Removed by Filter": 2,
                                              "Shared Hit Blocks": 1,
                                              "Shared Read Blocks": 0,
                                              "Shared Dirtied Blocks": 0,
                                              "Shared Written Blocks": 0,
                                              "Local Hit Blocks": 0,
                                              "Local Read Blocks": 0,
                                              "Local Dirtied Blocks": 0,
                                              "Local Written Blocks": 0,
                                              "Temp Read Blocks": 0,
                                              "Temp Written Blocks": 0
                                            }
                                          ]
                                        }
                                      ]
                                    },
                                    {
                                      "Node Type": "CTE Scan",
                                      "Parent Relationship": "SubPlan",
                                      "Subplan Name": "SubPlan 17",
                                      "Parallel Aware": false,
                                      "Async Capable": false,
                                      "CTE Name": "policy_products",
                                      "Alias": "pl",
                                      "Startup Cost": 0.0,
                                      "Total Cost": 2.25,
                                      "Plan Rows": 1,
                                      "Plan Width": 16,
                                      "Actual Startup Time": 0.094,
                                      "Actual Total Time": 0.103,
                                      "Actual Rows": 1,
                                      "Actual Loops": 1,
                                      "Filter": "(qualification_profile = 'lng_v1'::text)",
                                      "Rows Removed by Filter": 5,
                                      "Shared Hit Blocks": 0,
                                      "Shared Read Blocks": 0,
                                      "Shared Dirtied Blocks": 0,
                                      "Shared Written Blocks": 0,
                                      "Local Hit Blocks": 0,
                                      "Local Read Blocks": 0,
                                      "Local Dirtied Blocks": 0,
                                      "Local Written Blocks": 0,
                                      "Temp Read Blocks": 0,
                                      "Temp Written Blocks": 0
                                    },
                                    {
                                      "Node Type": "CTE Scan",
                                      "Parent Relationship": "SubPlan",
                                      "Subplan Name": "SubPlan 19",
                                      "Parallel Aware": false,
                                      "Async Capable": false,
                                      "CTE Name": "policy_products",
                                      "Alias": "pl_1",
                                      "Startup Cost": 0.0,
                                      "Total Cost": 177.75,
                                      "Plan Rows": 1,
                                      "Plan Width": 0,
                                      "Actual Startup Time": 0.0,
                                      "Actual Total Time": 0.0,
                                      "Actual Rows": 0,
                                      "Actual Loops": 0,
                                      "Filter": "((qualification_profile = 'lng_v1'::text) AND (id = o_1.product_id) AND EXISTS(SubPlan 18))",
                                      "Rows Removed by Filter": 0,
                                      "Shared Hit Blocks": 0,
                                      "Shared Read Blocks": 0,
                                      "Shared Dirtied Blocks": 0,
                                      "Shared Written Blocks": 0,
                                      "Local Hit Blocks": 0,
                                      "Local Read Blocks": 0,
                                      "Local Dirtied Blocks": 0,
                                      "Local Written Blocks": 0,
                                      "Temp Read Blocks": 0,
                                      "Temp Written Blocks": 0,
                                      "Plans": [
                                        {
                                          "Node Type": "Function Scan",
                                          "Parent Relationship": "SubPlan",
                                          "Subplan Name": "SubPlan 18",
                                          "Parallel Aware": false,
                                          "Async Capable": false,
                                          "Function Name": "jsonb_array_elements_text",
                                          "Alias": "lane_1",
                                          "Startup Cost": 0.0,
                                          "Total Cost": 1.75,
                                          "Plan Rows": 1,
                                          "Plan Width": 0,
                                          "Actual Startup Time": 0.0,
                                          "Actual Total Time": 0.0,
                                          "Actual Rows": 0,
                                          "Actual Loops": 0,
                                          "Filter": "(value = (o_1.delivery_point_id)::text)",
                                          "Rows Removed by Filter": 0,
                                          "Shared Hit Blocks": 0,
                                          "Shared Read Blocks": 0,
                                          "Shared Dirtied Blocks": 0,
                                          "Shared Written Blocks": 0,
                                          "Local Hit Blocks": 0,
                                          "Local Read Blocks": 0,
                                          "Local Dirtied Blocks": 0,
                                          "Local Written Blocks": 0,
                                          "Temp Read Blocks": 0,
                                          "Temp Written Blocks": 0
                                        }
                                      ]
                                    },
                                    {
                                      "Node Type": "CTE Scan",
                                      "Parent Relationship": "SubPlan",
                                      "Subplan Name": "SubPlan 13",
                                      "Parallel Aware": false,
                                      "Async Capable": false,
                                      "CTE Name": "policy_products",
                                      "Alias": "pf",
                                      "Startup Cost": 0.0,
                                      "Total Cost": 2.25,
                                      "Plan Rows": 1,
                                      "Plan Width": 16,
                                      "Actual Startup Time": 0.003,
                                      "Actual Total Time": 0.005,
                                      "Actual Rows": 1,
                                      "Actual Loops": 1,
                                      "Filter": "(qualification_profile = 'fame_v1'::text)",
                                      "Rows Removed by Filter": 5,
                                      "Shared Hit Blocks": 0,
                                      "Shared Read Blocks": 0,
                                      "Shared Dirtied Blocks": 0,
                                      "Shared Written Blocks": 0,
                                      "Local Hit Blocks": 0,
                                      "Local Read Blocks": 0,
                                      "Local Dirtied Blocks": 0,
                                      "Local Written Blocks": 0,
                                      "Temp Read Blocks": 0,
                                      "Temp Written Blocks": 0
                                    },
                                    {
                                      "Node Type": "CTE Scan",
                                      "Parent Relationship": "SubPlan",
                                      "Subplan Name": "SubPlan 15",
                                      "Parallel Aware": false,
                                      "Async Capable": false,
                                      "CTE Name": "policy_products",
                                      "Alias": "pf_1",
                                      "Startup Cost": 0.0,
                                      "Total Cost": 177.75,
                                      "Plan Rows": 1,
                                      "Plan Width": 0,
                                      "Actual Startup Time": 0.0,
                                      "Actual Total Time": 0.0,
                                      "Actual Rows": 0,
                                      "Actual Loops": 0,
                                      "Filter": "((qualification_profile = 'fame_v1'::text) AND (id = o_1.product_id) AND EXISTS(SubPlan 14))",
                                      "Rows Removed by Filter": 0,
                                      "Shared Hit Blocks": 0,
                                      "Shared Read Blocks": 0,
                                      "Shared Dirtied Blocks": 0,
                                      "Shared Written Blocks": 0,
                                      "Local Hit Blocks": 0,
                                      "Local Read Blocks": 0,
                                      "Local Dirtied Blocks": 0,
                                      "Local Written Blocks": 0,
                                      "Temp Read Blocks": 0,
                                      "Temp Written Blocks": 0,
                                      "Plans": [
                                        {
                                          "Node Type": "Function Scan",
                                          "Parent Relationship": "SubPlan",
                                          "Subplan Name": "SubPlan 14",
                                          "Parallel Aware": false,
                                          "Async Capable": false,
                                          "Function Name": "jsonb_array_elements_text",
                                          "Alias": "lane",
                                          "Startup Cost": 0.0,
                                          "Total Cost": 1.75,
                                          "Plan Rows": 1,
                                          "Plan Width": 0,
                                          "Actual Startup Time": 0.0,
                                          "Actual Total Time": 0.0,
                                          "Actual Rows": 0,
                                          "Actual Loops": 0,
                                          "Filter": "(value = (o_1.delivery_point_id)::text)",
                                          "Rows Removed by Filter": 0,
                                          "Shared Hit Blocks": 0,
                                          "Shared Read Blocks": 0,
                                          "Shared Dirtied Blocks": 0,
                                          "Shared Written Blocks": 0,
                                          "Local Hit Blocks": 0,
                                          "Local Read Blocks": 0,
                                          "Local Dirtied Blocks": 0,
                                          "Local Written Blocks": 0,
                                          "Temp Read Blocks": 0,
                                          "Temp Written Blocks": 0
                                        }
                                      ]
                                    }
                                  ]
                                },
                                {
                                  "Node Type": "Materialize",
                                  "Parent Relationship": "Inner",
                                  "Parallel Aware": false,
                                  "Async Capable": false,
                                  "Startup Cost": 0.0,
                                  "Total Cost": 1.13,
                                  "Plan Rows": 8,
                                  "Plan Width": 31,
                                  "Actual Startup Time": 0.0,
                                  "Actual Total Time": 0.0,
                                  "Actual Rows": 1,
                                  "Actual Loops": 10027,
                                  "Shared Hit Blocks": 1,
                                  "Shared Read Blocks": 0,
                                  "Shared Dirtied Blocks": 0,
                                  "Shared Written Blocks": 0,
                                  "Local Hit Blocks": 0,
                                  "Local Read Blocks": 0,
                                  "Local Dirtied Blocks": 0,
                                  "Local Written Blocks": 0,
                                  "Temp Read Blocks": 0,
                                  "Temp Written Blocks": 0,
                                  "Plans": [
                                    {
                                      "Node Type": "Seq Scan",
                                      "Parent Relationship": "Outer",
                                      "Parallel Aware": false,
                                      "Async Capable": false,
                                      "Relation Name": "delivery_points",
                                      "Alias": "dp",
                                      "Startup Cost": 0.0,
                                      "Total Cost": 1.09,
                                      "Plan Rows": 8,
                                      "Plan Width": 31,
                                      "Actual Startup Time": 0.012,
                                      "Actual Total Time": 0.014,
                                      "Actual Rows": 8,
                                      "Actual Loops": 1,
                                      "Filter": "is_active",
                                      "Rows Removed by Filter": 1,
                                      "Shared Hit Blocks": 1,
                                      "Shared Read Blocks": 0,
                                      "Shared Dirtied Blocks": 0,
                                      "Shared Written Blocks": 0,
                                      "Local Hit Blocks": 0,
                                      "Local Read Blocks": 0,
                                      "Local Dirtied Blocks": 0,
                                      "Local Written Blocks": 0,
                                      "Temp Read Blocks": 0,
                                      "Temp Written Blocks": 0
                                    }
                                  ]
                                }
                              ]
                            },
                            {
                              "Node Type": "Materialize",
                              "Parent Relationship": "Inner",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Startup Cost": 0.0,
                              "Total Cost": 1.14,
                              "Plan Rows": 6,
                              "Plan Width": 41,
                              "Actual Startup Time": 0.0,
                              "Actual Total Time": 0.0,
                              "Actual Rows": 2,
                              "Actual Loops": 10026,
                              "Shared Hit Blocks": 1,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0,
                              "Plans": [
                                {
                                  "Node Type": "Seq Scan",
                                  "Parent Relationship": "Outer",
                                  "Parallel Aware": false,
                                  "Async Capable": false,
                                  "Relation Name": "products",
                                  "Alias": "p",
                                  "Startup Cost": 0.0,
                                  "Total Cost": 1.1,
                                  "Plan Rows": 6,
                                  "Plan Width": 41,
                                  "Actual Startup Time": 0.009,
                                  "Actual Total Time": 0.013,
                                  "Actual Rows": 6,
                                  "Actual Loops": 1,
                                  "Filter": "(is_active AND ((fuel_type)::text = ANY ('{Methanol,Ethanol,FAME,LNG}'::text[])))",
                                  "Rows Removed by Filter": 1,
                                  "Shared Hit Blocks": 1,
                                  "Shared Read Blocks": 0,
                                  "Shared Dirtied Blocks": 0,
                                  "Shared Written Blocks": 0,
                                  "Local Hit Blocks": 0,
                                  "Local Read Blocks": 0,
                                  "Local Dirtied Blocks": 0,
                                  "Local Written Blocks": 0,
                                  "Temp Read Blocks": 0,
                                  "Temp Written Blocks": 0
                                }
                              ]
                            },
                            {
                              "Node Type": "CTE Scan",
                              "Parent Relationship": "SubPlan",
                              "Subplan Name": "SubPlan 21",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "CTE Name": "policy_products",
                              "Alias": "pd",
                              "Startup Cost": 0.0,
                              "Total Cost": 2.25,
                              "Plan Rows": 2,
                              "Plan Width": 112,
                              "Actual Startup Time": 0.0,
                              "Actual Total Time": 0.0,
                              "Actual Rows": 0,
                              "Actual Loops": 0,
                              "Filter": "(qualification_profile = ANY ('{fame_v1,lng_v1}'::text[]))",
                              "Rows Removed by Filter": 0,
                              "Shared Hit Blocks": 0,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            },
                            {
                              "Node Type": "CTE Scan",
                              "Parent Relationship": "SubPlan",
                              "Subplan Name": "SubPlan 23",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "CTE Name": "policy_products",
                              "Alias": "po",
                              "Startup Cost": 0.0,
                              "Total Cost": 2.25,
                              "Plan Rows": 1,
                              "Plan Width": 112,
                              "Actual Startup Time": 0.0,
                              "Actual Total Time": 0.0,
                              "Actual Rows": 0,
                              "Actual Loops": 0,
                              "Filter": "(qualification_profile = 'fame_v1'::text)",
                              "Rows Removed by Filter": 0,
                              "Shared Hit Blocks": 0,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            }
                          ]
                        },
                        {
                          "Node Type": "CTE Scan",
                          "Parent Relationship": "Inner",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "CTE Name": "policy_products",
                          "Alias": "pp",
                          "Startup Cost": 0.0,
                          "Total Cost": 2.25,
                          "Plan Rows": 1,
                          "Plan Width": 144,
                          "Actual Startup Time": 0.001,
                          "Actual Total Time": 0.001,
                          "Actual Rows": 3,
                          "Actual Loops": 10025,
                          "Filter": "(execution_mode = 'ORDERBOOK'::text)",
                          "Rows Removed by Filter": 0,
                          "Shared Hit Blocks": 0,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0
                        },
                        {
                          "Node Type": "Function Scan",
                          "Parent Relationship": "SubPlan",
                          "Subplan Name": "SubPlan 24",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Function Name": "jsonb_array_elements_text",
                          "Alias": "lane_2",
                          "Startup Cost": 0.0,
                          "Total Cost": 1.75,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.007,
                          "Actual Total Time": 0.007,
                          "Actual Rows": 1,
                          "Actual Loops": 10024,
                          "Filter": "(value = (o_1.delivery_point_id)::text)",
                          "Rows Removed by Filter": 3,
                          "Shared Hit Blocks": 0,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0
                        }
                      ]
                    },
                    {
                      "Node Type": "Function Scan",
                      "Parent Relationship": "Inner",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Function Name": "jsonb_to_recordset",
                      "Alias": "policy_point",
                      "Startup Cost": 0.0,
                      "Total Cost": 1.0,
                      "Plan Rows": 100,
                      "Plan Width": 80,
                      "Actual Startup Time": 0.0,
                      "Actual Total Time": 0.0,
                      "Actual Rows": 4,
                      "Actual Loops": 10023,
                      "Shared Hit Blocks": 0,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0
                    },
                    {
                      "Node Type": "CTE Scan",
                      "Parent Relationship": "SubPlan",
                      "Subplan Name": "SubPlan 2",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "CTE Name": "policy_products",
                      "Alias": "pp_1",
                      "Startup Cost": 0.0,
                      "Total Cost": 2.25,
                      "Plan Rows": 1,
                      "Plan Width": 32,
                      "Actual Startup Time": 0.001,
                      "Actual Total Time": 0.002,
                      "Actual Rows": 1,
                      "Actual Loops": 10023,
                      "Filter": "(id = p.id)",
                      "Rows Removed by Filter": 5,
                      "Shared Hit Blocks": 0,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0
                    }
                  ]
                }
              ]
            }
          ]
        }
      ]
    },
    {
      "Node Type": "Seq Scan",
      "Parent Relationship": "Inner",
      "Parallel Aware": false,
      "Async Capable": false,
      "Relation Name": "organizations",
      "Alias": "org",
      "Startup Cost": 0.0,
      "Total Cost": 1.13,
      "Plan Rows": 13,
      "Plan Width": 134,
      "Actual Startup Time": 0.002,
      "Actual Total Time": 0.003,
      "Actual Rows": 4,
      "Actual Loops": 20,
      "Shared Hit Blocks": 20,
      "Shared Read Blocks": 0,
      "Shared Dirtied Blocks": 0,
      "Shared Written Blocks": 0,
      "Local Hit Blocks": 0,
      "Local Read Blocks": 0,
      "Local Dirtied Blocks": 0,
      "Local Written Blocks": 0,
      "Temp Read Blocks": 0,
      "Temp Written Blocks": 0
    }
  ]
}
```

### rust /api/orderbook/bids rows=10000

```sql
WITH eligible_orders AS (-- Shared public-order visibility projection (Task 6).
--
-- This is the complete bound internal relation: every caller value is a parameter, so
-- no route ever concatenates text into SQL. Parameters are
--   $1  attested market policy (jsonb text)
--   $2  captured UTC instant (timestamptz)
--   $3  Singapore date (text, YYYY-MM-DD)
--   $4  include_off_spec (boolean)
--   $5  product_id (uuid, null = any)
--   $6  delivery_point_id (uuid, null = any)
--   $7  side (text, null = any)
--   $8  fuel_type (text, null = any)
--   $9  market_product code (text, null = any)
--   $10 region (text, null = any)
--   $11 availability_window (text, null = any)
--   $12 surface: orderbook, listings, or candidates (text)
--   $13 runtime KYC execution gate enabled (boolean)
-- No pagination here: consumers rank, aggregate and page the shared relation.
--
-- Product identity comes only from the policy. The statement never names a
-- product, a product code, or a product UUID.
WITH policy_products AS (
    SELECT *
    FROM jsonb_to_recordset(($1::jsonb) -> 'products') AS policy_product(
        id uuid,
        code text,
        name text,
        fuel_type text,
        fuel_grade text,
        execution_mode text,
        lanes jsonb,
        qualification_profile text,
        display_order integer
    )
),
policy_points AS (
    SELECT *
    FROM jsonb_to_recordset(($1::jsonb) -> 'points') AS policy_point(
        id uuid,
        name text,
        region text,
        timezone text,
        display_order integer
    )
)
SELECT o.*, p.name AS product_name, p.fuel_type AS product_fuel_type,
       p.fuel_grade AS product_fuel_grade, p.is_active AS product_is_active,
       dp.name AS delivery_point_name, dp.region AS delivery_point_region,
       (SELECT pp.code FROM policy_products pp WHERE pp.id = p.id) AS market_product,
       -- Internal only. Never serialize this object as a public DTO.
       to_jsonb(o) || jsonb_build_object(
           'quantity_mt', o.quantity_mt::text,
           'remaining_quantity_mt', o.remaining_quantity_mt::text,
           'price_per_mt_usd', o.price_per_mt_usd::text,
           'carbon_intensity_gco2_mj', o.carbon_intensity_gco2_mj::text,
           'energy_density_mj_kg', o.energy_density_mj_kg::text
       ) AS private_digest_input
FROM orderbook_orders AS o
JOIN products AS p ON p.id = o.product_id
JOIN delivery_points AS dp ON dp.id = o.delivery_point_id
WHERE o.status IN ('OPEN', 'PARTIALLY_FILLED')
    -- Strict canonical product identity, active row, and approved lane.
    AND p.is_active
    AND EXISTS (
        SELECT 1
        FROM policy_products AS pp
        WHERE pp.execution_mode = 'ORDERBOOK'
          AND pp.id = p.id
          AND pp.name = p.name
          AND pp.fuel_type = p.fuel_type
          AND pp.fuel_grade = p.fuel_grade
          AND EXISTS (
              SELECT 1
              FROM jsonb_array_elements_text(pp.lanes) AS lane(value)
              WHERE lane.value = o.delivery_point_id::text
          )
    )
    -- Strict canonical delivery-point identity and active row.
    AND dp.is_active
    AND EXISTS (
        SELECT 1
        FROM policy_points AS pq
        WHERE pq.id = dp.id AND pq.name = dp.name AND pq.region = dp.region
    )
    -- Canonical availability-window grammar.
    AND ($12::text <> 'orderbook' OR (
        o.availability_window = 'SPOT'
        OR o.availability_window ~ '^[0-9]{4}-(0[1-9]|1[0-2])$'
        OR o.availability_window ~ '^[0-9]{4}-Q[1-4]$'
        OR o.availability_window ~ '^[0-9]{4}-CAL$'
    ))
    AND (-- Shared expiry, owner admission and shallow declaration rule.
-- Requires policy_products and alias o. Binds $2, $3 and $13.
TRUE
    -- Currency: an explicit future expiry, or a null expiry on REAL/UNKNOWN.
    AND (
        o.expires_at > $2::timestamptz
        OR (
            o.expires_at IS NULL
            AND o.provenance IN ('REAL', 'UNKNOWN')
        )
    )
    -- Owner admission. Assisted rows use the narrower administrator rule.
    AND (
        CASE
            WHEN o.creation_method = 'MARKET_SUPPORT' THEN EXISTS (
                SELECT 1
                FROM users AS actor
                WHERE actor.id = o.owner_user_id
                  AND actor.id = o.created_by_actor_user_id
                  AND actor.role = 'ADMIN'
                  AND actor.status = 'APPROVED'
                  AND EXISTS (
                      SELECT 1
                      FROM users AS member
                      WHERE member.organization_id = o.organization_id
                        AND member.role IN ('BUYER', 'SUPPLIER')
                        AND member.status = 'APPROVED'
                        AND member.email_verified
                        AND NOT member.must_change_password
                        AND member.kyc_status <> 'REJECTED'
                        AND CASE WHEN $13::boolean THEN (
                            (member.kyc_status = 'APPROVED'
                             AND member.kyc_organization_id = member.organization_id)
                            OR EXISTS (
                                SELECT 1 FROM kyc_execution_grace AS grace
                                WHERE grace.user_id = member.id
                                  AND grace.organization_id = member.organization_id
                                  AND grace.expires_at > $2::timestamptz
                            )
                        ) ELSE (
                            member.kyc_organization_id IS NULL
                            OR member.kyc_organization_id = member.organization_id
                        ) END
                  )
                  AND EXISTS (
                      SELECT 1
                      FROM organizations AS assisted_org
                      WHERE assisted_org.id = o.organization_id
                        AND assisted_org.verification_status = 'APPROVED'
                        AND assisted_org.provenance = 'REAL'
                  )
            )
            ELSE (
                o.owner_user_id IS NULL
                OR EXISTS (
                    SELECT 1
                    FROM users AS owner
                    WHERE owner.id = o.owner_user_id
                      AND owner.organization_id = o.organization_id
                      AND owner.role IN ('BUYER', 'SUPPLIER')
                      AND owner.status = 'APPROVED'
                      AND owner.email_verified
                      AND NOT owner.must_change_password
                      AND owner.kyc_status <> 'REJECTED'
                      AND CASE WHEN $13::boolean THEN (
                          (owner.kyc_status = 'APPROVED'
                           AND owner.kyc_organization_id = owner.organization_id)
                          OR EXISTS (
                              SELECT 1 FROM kyc_execution_grace AS grace
                              WHERE grace.user_id = owner.id
                                AND grace.organization_id = owner.organization_id
                                AND grace.expires_at > $2::timestamptz
                          )
                      ) ELSE (
                          owner.kyc_organization_id IS NULL
                          OR owner.kyc_organization_id = owner.organization_id
                      ) END
                      AND EXISTS (
                          SELECT 1
                          FROM organizations AS org
                          WHERE org.id = o.organization_id
                            AND org.verification_status = 'APPROVED'
                      )
                )
            )
        END
    )
    -- fame_v1 declaration currency, keyed by policy profile.
    AND (
        NOT EXISTS (
            SELECT 1
            FROM policy_products AS pf
            WHERE pf.qualification_profile = 'fame_v1'
              AND pf.id = o.product_id
        )
        OR (
            EXISTS (
                SELECT 1
                FROM policy_products AS pf
                WHERE pf.qualification_profile = 'fame_v1'
                  AND pf.id = o.product_id
                  AND EXISTS (
                      SELECT 1
                      FROM jsonb_array_elements_text(pf.lanes) AS lane(value)
                      WHERE lane.value = o.delivery_point_id::text
                  )
            )
            AND o.fame_terms ->> 'side' = o.side
            AND o.off_spec = false
            AND (
                o.side <> 'ASK'
                OR (
                    o.fame_terms ->> 'certificate_valid_until' >= $3::text
                    AND o.msds_available
                    AND o.certification_declared
                    AND length(btrim(coalesce(o.certification_scheme, ''))) > 0
                )
            )
        )
    )
    -- lng_v1 declaration currency, keyed by policy profile.
    AND (
        NOT EXISTS (
            SELECT 1
            FROM policy_products AS pl
            WHERE pl.qualification_profile = 'lng_v1'
              AND pl.id = o.product_id
        )
        OR (
            EXISTS (
                SELECT 1
                FROM policy_products AS pl
                WHERE pl.qualification_profile = 'lng_v1'
                  AND pl.id = o.product_id
                  AND EXISTS (
                      SELECT 1
                      FROM jsonb_array_elements_text(pl.lanes) AS lane(value)
                      WHERE lane.value = o.delivery_point_id::text
                  )
            )
            AND o.lng_terms ->> 'side' = o.side
            AND o.off_spec = false
        )
    )
)
    -- Independently labelled public rows.
    AND o.provenance IN ('REAL', 'DEMO')
    -- Admitted fuel types.
    AND p.fuel_type IN ('Methanol', 'Ethanol', 'FAME', 'LNG')
    -- Scalar ASK quality declarations.
    AND ($12::text <> 'orderbook' OR (
        o.side <> 'ASK'
        OR (
            length(btrim(coalesce(o.certification_scheme, ''))) > 0
            AND o.certification_declared
            AND length(btrim(coalesce(o.specification_standard, ''))) > 0
            AND o.msds_available
            AND (
                EXISTS (
                    SELECT 1
                    FROM policy_products AS pd
                    WHERE pd.qualification_profile IN ('fame_v1', 'lng_v1')
                      AND pd.id = p.id
                      AND pd.name = p.name
                      AND pd.fuel_type = p.fuel_type
                      AND pd.fuel_grade = p.fuel_grade
                )
                OR o.carbon_intensity_gco2_mj IS NOT NULL
            )
            AND length(btrim(coalesce(o.feedstock, ''))) > 0
            AND (
                EXISTS (
                    SELECT 1
                    FROM policy_products AS po
                    WHERE po.qualification_profile = 'fame_v1'
                      AND po.id = p.id
                      AND po.name = p.name
                      AND po.fuel_type = p.fuel_type
                      AND po.fuel_grade = p.fuel_grade
                )
                OR length(btrim(coalesce(o.origin, ''))) > 0
            )
        )
    )
    )
    -- Off-spec rows are hidden unless the caller asks for them.
    AND ($12::text <> 'orderbook' OR o.off_spec = false OR $4::boolean)
    AND ($12::text <> 'listings' OR o.side = 'ASK')
    AND $12::text IN ('orderbook', 'listings', 'candidates')
    -- Route filters.
    AND ($5::uuid IS NULL OR o.product_id = $5::uuid)
    AND ($6::uuid IS NULL OR o.delivery_point_id = $6::uuid)
    AND ($7::text IS NULL OR o.side = $7::text)
    AND ($8::text IS NULL OR p.fuel_type = $8::text)
    AND (
        $9::text IS NULL
        OR EXISTS (
            SELECT 1
            FROM policy_products AS pm
            WHERE pm.code = $9::text
              AND pm.id = p.id
              AND pm.name = p.name
              AND pm.fuel_type = p.fuel_type
              AND pm.fuel_grade = p.fuel_grade
        )
    )
    AND ($10::text IS NULL OR $10::text = '' OR dp.region = $10::text OR dp.name = $10::text)
    AND ($11::text IS NULL OR o.availability_window = $11::text)
) , requested_keys AS MATERIALIZED (
    SELECT DISTINCT side, product, point, "window"
    FROM jsonb_to_recordset($14::jsonb) AS requested(
        side text, product text, point uuid, "window" text
    )
)
SELECT o.side, o.market_product, o.delivery_point_id, o.availability_window,
       o.remaining_quantity_mt, o.price_per_mt_usd, o.private_digest_input
FROM eligible_orders AS o
JOIN requested_keys AS requested
    ON (o.side, o.market_product, o.delivery_point_id, o.availability_window)
        = (requested.side, requested.product, requested.point, requested."window")

```

```json
{
  "Node Type": "Nested Loop",
  "Parallel Aware": false,
  "Async Capable": false,
  "Join Type": "Inner",
  "Startup Cost": 4.01,
  "Total Cost": 3897781.65,
  "Plan Rows": 1,
  "Plan Width": 100,
  "Actual Startup Time": 12.666,
  "Actual Total Time": 2629.365,
  "Actual Rows": 10000,
  "Actual Loops": 1,
  "Inner Unique": false,
  "Join Filter": "(((o.side)::text = requested.side) AND (((SubPlan 3)) = requested.product) AND (o.delivery_point_id = requested.point) AND ((o.availability_window)::text = requested.\"window\"))",
  "Rows Removed by Join Filter": 47,
  "Shared Hit Blocks": 30473,
  "Shared Read Blocks": 0,
  "Shared Dirtied Blocks": 0,
  "Shared Written Blocks": 0,
  "Local Hit Blocks": 0,
  "Local Read Blocks": 0,
  "Local Dirtied Blocks": 0,
  "Local Written Blocks": 0,
  "Temp Read Blocks": 0,
  "Temp Written Blocks": 0,
  "Plans": [
    {
      "Node Type": "Aggregate",
      "Strategy": "Hashed",
      "Partial Mode": "Simple",
      "Parent Relationship": "InitPlan",
      "Subplan Name": "CTE requested_keys",
      "Parallel Aware": false,
      "Async Capable": false,
      "Startup Cost": 2.0,
      "Total Cost": 3.0,
      "Plan Rows": 100,
      "Plan Width": 112,
      "Actual Startup Time": 0.126,
      "Actual Total Time": 0.128,
      "Actual Rows": 1,
      "Actual Loops": 1,
      "Group Key": [
        "requested_1.side",
        "requested_1.product",
        "requested_1.point",
        "requested_1.\"window\""
      ],
      "Planned Partitions": 0,
      "HashAgg Batches": 1,
      "Peak Memory Usage": 32,
      "Disk Usage": 0,
      "Shared Hit Blocks": 0,
      "Shared Read Blocks": 0,
      "Shared Dirtied Blocks": 0,
      "Shared Written Blocks": 0,
      "Local Hit Blocks": 0,
      "Local Read Blocks": 0,
      "Local Dirtied Blocks": 0,
      "Local Written Blocks": 0,
      "Temp Read Blocks": 0,
      "Temp Written Blocks": 0,
      "Plans": [
        {
          "Node Type": "Function Scan",
          "Parent Relationship": "Outer",
          "Parallel Aware": false,
          "Async Capable": false,
          "Function Name": "jsonb_to_recordset",
          "Alias": "requested_1",
          "Startup Cost": 0.0,
          "Total Cost": 1.0,
          "Plan Rows": 100,
          "Plan Width": 112,
          "Actual Startup Time": 0.052,
          "Actual Total Time": 0.055,
          "Actual Rows": 20,
          "Actual Loops": 1,
          "Shared Hit Blocks": 0,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0
        }
      ]
    },
    {
      "Node Type": "Nested Loop",
      "Parent Relationship": "Outer",
      "Parallel Aware": false,
      "Async Capable": false,
      "Join Type": "Semi",
      "Startup Cost": 1.0,
      "Total Cost": 3897774.64,
      "Plan Rows": 1,
      "Plan Width": 3541,
      "Actual Startup Time": 7.897,
      "Actual Total Time": 2599.944,
      "Actual Rows": 10047,
      "Actual Loops": 1,
      "Inner Unique": false,
      "Join Filter": "((policy_point.id = dp.id) AND ((dp.name)::text = policy_point.name) AND (policy_point.region = (dp.region)::text))",
      "Rows Removed by Join Filter": 30145,
      "Shared Hit Blocks": 30473,
      "Shared Read Blocks": 0,
      "Shared Dirtied Blocks": 0,
      "Shared Written Blocks": 0,
      "Local Hit Blocks": 0,
      "Local Read Blocks": 0,
      "Local Dirtied Blocks": 0,
      "Local Written Blocks": 0,
      "Temp Read Blocks": 0,
      "Temp Written Blocks": 0,
      "Plans": [
        {
          "Node Type": "Function Scan",
          "Parent Relationship": "InitPlan",
          "Subplan Name": "CTE policy_products",
          "Parallel Aware": false,
          "Async Capable": false,
          "Function Name": "jsonb_to_recordset",
          "Alias": "policy_product",
          "Startup Cost": 0.0,
          "Total Cost": 1.0,
          "Plan Rows": 100,
          "Plan Width": 244,
          "Actual Startup Time": 0.107,
          "Actual Total Time": 0.109,
          "Actual Rows": 6,
          "Actual Loops": 1,
          "Shared Hit Blocks": 0,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0
        },
        {
          "Node Type": "Nested Loop",
          "Parent Relationship": "Outer",
          "Parallel Aware": false,
          "Async Capable": false,
          "Join Type": "Semi",
          "Startup Cost": 0.0,
          "Total Cost": 3897768.6,
          "Plan Rows": 1,
          "Plan Width": 3034,
          "Actual Startup Time": 7.702,
          "Actual Total Time": 501.056,
          "Actual Rows": 10047,
          "Actual Loops": 1,
          "Inner Unique": false,
          "Join Filter": "((pp.id = p.id) AND ((p.name)::text = pp.name) AND (pp.fuel_type = (p.fuel_type)::text) AND (pp.fuel_grade = (p.fuel_grade)::text) AND EXISTS(SubPlan 21))",
          "Rows Removed by Join Filter": 20095,
          "Shared Hit Blocks": 30473,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0,
          "Plans": [
            {
              "Node Type": "Nested Loop",
              "Parent Relationship": "Outer",
              "Parallel Aware": false,
              "Async Capable": false,
              "Join Type": "Inner",
              "Startup Cost": 0.0,
              "Total Cost": 3896049.81,
              "Plan Rows": 963,
              "Plan Width": 3074,
              "Actual Startup Time": 7.659,
              "Actual Total Time": 334.499,
              "Actual Rows": 10049,
              "Actual Loops": 1,
              "Inner Unique": true,
              "Join Filter": "(o.delivery_point_id = dp.id)",
              "Rows Removed by Join Filter": 31,
              "Shared Hit Blocks": 30473,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0,
              "Plans": [
                {
                  "Node Type": "Nested Loop",
                  "Parent Relationship": "Outer",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Join Type": "Inner",
                  "Startup Cost": 0.0,
                  "Total Cost": 3895929.94,
                  "Plan Rows": 1083,
                  "Plan Width": 3043,
                  "Actual Startup Time": 7.62,
                  "Actual Total Time": 297.361,
                  "Actual Rows": 10050,
                  "Actual Loops": 1,
                  "Inner Unique": true,
                  "Join Filter": "(o.product_id = p.id)",
                  "Rows Removed by Join Filter": 10078,
                  "Shared Hit Blocks": 30472,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0,
                  "Plans": [
                    {
                      "Node Type": "Seq Scan",
                      "Parent Relationship": "Outer",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Relation Name": "orderbook_orders",
                      "Alias": "o",
                      "Startup Cost": 0.0,
                      "Total Cost": 3895826.7,
                      "Plan Rows": 1264,
                      "Plan Width": 3003,
                      "Actual Startup Time": 7.559,
                      "Actual Total Time": 281.38,
                      "Actual Rows": 10051,
                      "Actual Loops": 1,
                      "Filter": "(((status)::text = ANY ('{OPEN,PARTIALLY_FILLED}'::text[])) AND ((provenance)::text = ANY ('{REAL,DEMO}'::text[])) AND ((expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone) OR ((expires_at IS NULL) AND ((provenance)::text = ANY ('{REAL,UNKNOWN}'::text[])))) AND CASE WHEN ((creation_method)::text = 'MARKET_SUPPORT'::text) THEN EXISTS(SubPlan 8) ELSE ((owner_user_id IS NULL) OR EXISTS(SubPlan 12)) END AND ((NOT (ANY (product_id = (hashed SubPlan 18).col1))) OR (EXISTS(SubPlan 20) AND ((lng_terms ->> 'side'::text) = (side)::text) AND (NOT off_spec))) AND ((NOT (ANY (product_id = (hashed SubPlan 14).col1))) OR (EXISTS(SubPlan 16) AND ((fame_terms ->> 'side'::text) = (side)::text) AND (NOT off_spec) AND (((side)::text <> 'ASK'::text) OR (((fame_terms ->> 'certificate_valid_until'::text) >= '2026-10-15'::text) AND msds_available AND certification_declared AND (length(btrim((COALESCE(certification_scheme, ''::character varying))::text)) > 0))))))",
                      "Rows Removed by Filter": 28,
                      "Shared Hit Blocks": 30471,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0,
                      "Plans": [
                        {
                          "Node Type": "Result",
                          "Parent Relationship": "SubPlan",
                          "Subplan Name": "SubPlan 8",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Startup Cost": 14.05,
                          "Total Cost": 15.45,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.023,
                          "Actual Total Time": 0.024,
                          "Actual Rows": 0,
                          "Actual Loops": 7,
                          "One-Time Filter": "((InitPlan 6).col1 AND (InitPlan 7).col1 AND (o.owner_user_id = o.created_by_actor_user_id))",
                          "Shared Hit Blocks": 22,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0,
                          "Plans": [
                            {
                              "Node Type": "Index Scan",
                              "Parent Relationship": "InitPlan",
                              "Subplan Name": "InitPlan 6",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Scan Direction": "Forward",
                              "Index Name": "ix_users_organization_id_id",
                              "Relation Name": "users",
                              "Alias": "member",
                              "Startup Cost": 0.14,
                              "Total Cost": 12.82,
                              "Plan Rows": 1,
                              "Plan Width": 0,
                              "Actual Startup Time": 0.013,
                              "Actual Total Time": 0.014,
                              "Actual Rows": 1,
                              "Actual Loops": 7,
                              "Index Cond": "(organization_id = o.organization_id)",
                              "Rows Removed by Index Recheck": 0,
                              "Filter": "(email_verified AND (NOT must_change_password) AND ((role)::text = ANY ('{BUYER,SUPPLIER}'::text[])) AND ((kyc_status)::text <> 'REJECTED'::text) AND ((status)::text = 'APPROVED'::text) AND ((((kyc_status)::text = 'APPROVED'::text) AND (kyc_organization_id = organization_id)) OR (ANY ((id = (hashed SubPlan 5).col1) AND (organization_id = (hashed SubPlan 5).col2)))))",
                              "Rows Removed by Filter": 0,
                              "Shared Hit Blocks": 15,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0,
                              "Plans": [
                                {
                                  "Node Type": "Seq Scan",
                                  "Parent Relationship": "SubPlan",
                                  "Subplan Name": "SubPlan 5",
                                  "Parallel Aware": false,
                                  "Async Capable": false,
                                  "Relation Name": "kyc_execution_grace",
                                  "Alias": "grace",
                                  "Startup Cost": 0.0,
                                  "Total Cost": 1.1,
                                  "Plan Rows": 6,
                                  "Plan Width": 32,
                                  "Actual Startup Time": 0.009,
                                  "Actual Total Time": 0.011,
                                  "Actual Rows": 6,
                                  "Actual Loops": 1,
                                  "Filter": "(expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone)",
                                  "Rows Removed by Filter": 2,
                                  "Shared Hit Blocks": 1,
                                  "Shared Read Blocks": 0,
                                  "Shared Dirtied Blocks": 0,
                                  "Shared Written Blocks": 0,
                                  "Local Hit Blocks": 0,
                                  "Local Read Blocks": 0,
                                  "Local Dirtied Blocks": 0,
                                  "Local Written Blocks": 0,
                                  "Temp Read Blocks": 0,
                                  "Temp Written Blocks": 0
                                }
                              ]
                            },
                            {
                              "Node Type": "Seq Scan",
                              "Parent Relationship": "InitPlan",
                              "Subplan Name": "InitPlan 7",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Relation Name": "organizations",
                              "Alias": "assisted_org",
                              "Startup Cost": 0.0,
                              "Total Cost": 1.23,
                              "Plan Rows": 1,
                              "Plan Width": 0,
                              "Actual Startup Time": 0.003,
                              "Actual Total Time": 0.004,
                              "Actual Rows": 1,
                              "Actual Loops": 4,
                              "Filter": "((id = o.organization_id) AND ((verification_status)::text = 'APPROVED'::text) AND ((provenance)::text = 'REAL'::text))",
                              "Rows Removed by Filter": 4,
                              "Shared Hit Blocks": 4,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            },
                            {
                              "Node Type": "Seq Scan",
                              "Parent Relationship": "Outer",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Relation Name": "users",
                              "Alias": "actor",
                              "Startup Cost": 14.05,
                              "Total Cost": 15.45,
                              "Plan Rows": 1,
                              "Plan Width": 0,
                              "Actual Startup Time": 0.012,
                              "Actual Total Time": 0.012,
                              "Actual Rows": 1,
                              "Actual Loops": 3,
                              "Filter": "((id = o.created_by_actor_user_id) AND ((role)::text = 'ADMIN'::text) AND ((status)::text = 'APPROVED'::text))",
                              "Rows Removed by Filter": 13,
                              "Shared Hit Blocks": 3,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            }
                          ]
                        },
                        {
                          "Node Type": "Result",
                          "Parent Relationship": "SubPlan",
                          "Subplan Name": "SubPlan 12",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Startup Cost": 1.33,
                          "Total Cost": 10.5,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.017,
                          "Actual Total Time": 0.017,
                          "Actual Rows": 1,
                          "Actual Loops": 10061,
                          "One-Time Filter": "(InitPlan 11).col1",
                          "Shared Hit Blocks": 30181,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0,
                          "Plans": [
                            {
                              "Node Type": "Seq Scan",
                              "Parent Relationship": "InitPlan",
                              "Subplan Name": "InitPlan 11",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Relation Name": "organizations",
                              "Alias": "org",
                              "Startup Cost": 0.0,
                              "Total Cost": 1.2,
                              "Plan Rows": 1,
                              "Plan Width": 0,
                              "Actual Startup Time": 0.004,
                              "Actual Total Time": 0.004,
                              "Actual Rows": 1,
                              "Actual Loops": 10061,
                              "Filter": "((id = o.organization_id) AND ((verification_status)::text = 'APPROVED'::text))",
                              "Rows Removed by Filter": 3,
                              "Shared Hit Blocks": 10061,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            },
                            {
                              "Node Type": "Index Scan",
                              "Parent Relationship": "Outer",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Scan Direction": "Forward",
                              "Index Name": "ix_users_organization_id_id",
                              "Relation Name": "users",
                              "Alias": "owner",
                              "Startup Cost": 1.33,
                              "Total Cost": 10.5,
                              "Plan Rows": 1,
                              "Plan Width": 0,
                              "Actual Startup Time": 0.007,
                              "Actual Total Time": 0.007,
                              "Actual Rows": 1,
                              "Actual Loops": 10060,
                              "Index Cond": "((organization_id = o.organization_id) AND (id = o.owner_user_id))",
                              "Rows Removed by Index Recheck": 0,
                              "Filter": "(email_verified AND (NOT must_change_password) AND ((role)::text = ANY ('{BUYER,SUPPLIER}'::text[])) AND ((kyc_status)::text <> 'REJECTED'::text) AND ((status)::text = 'APPROVED'::text) AND ((((kyc_status)::text = 'APPROVED'::text) AND (kyc_organization_id = organization_id)) OR (ANY ((id = (hashed SubPlan 10).col1) AND (organization_id = (hashed SubPlan 10).col2)))))",
                              "Rows Removed by Filter": 0,
                              "Shared Hit Blocks": 20120,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0,
                              "Plans": [
                                {
                                  "Node Type": "Seq Scan",
                                  "Parent Relationship": "SubPlan",
                                  "Subplan Name": "SubPlan 10",
                                  "Parallel Aware": false,
                                  "Async Capable": false,
                                  "Relation Name": "kyc_execution_grace",
                                  "Alias": "grace_1",
                                  "Startup Cost": 0.0,
                                  "Total Cost": 1.1,
                                  "Plan Rows": 6,
                                  "Plan Width": 32,
                                  "Actual Startup Time": 0.008,
                                  "Actual Total Time": 0.009,
                                  "Actual Rows": 6,
                                  "Actual Loops": 1,
                                  "Filter": "(expires_at > '2026-10-15 20:00:00+08'::timestamp with time zone)",
                                  "Rows Removed by Filter": 2,
                                  "Shared Hit Blocks": 1,
                                  "Shared Read Blocks": 0,
                                  "Shared Dirtied Blocks": 0,
                                  "Shared Written Blocks": 0,
                                  "Local Hit Blocks": 0,
                                  "Local Read Blocks": 0,
                                  "Local Dirtied Blocks": 0,
                                  "Local Written Blocks": 0,
                                  "Temp Read Blocks": 0,
                                  "Temp Written Blocks": 0
                                }
                              ]
                            }
                          ]
                        },
                        {
                          "Node Type": "CTE Scan",
                          "Parent Relationship": "SubPlan",
                          "Subplan Name": "SubPlan 18",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "CTE Name": "policy_products",
                          "Alias": "pl",
                          "Startup Cost": 0.0,
                          "Total Cost": 2.25,
                          "Plan Rows": 1,
                          "Plan Width": 16,
                          "Actual Startup Time": 0.113,
                          "Actual Total Time": 0.124,
                          "Actual Rows": 1,
                          "Actual Loops": 1,
                          "Filter": "(qualification_profile = 'lng_v1'::text)",
                          "Rows Removed by Filter": 5,
                          "Shared Hit Blocks": 0,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0
                        },
                        {
                          "Node Type": "CTE Scan",
                          "Parent Relationship": "SubPlan",
                          "Subplan Name": "SubPlan 20",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "CTE Name": "policy_products",
                          "Alias": "pl_1",
                          "Startup Cost": 0.0,
                          "Total Cost": 177.75,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.007,
                          "Actual Total Time": 0.008,
                          "Actual Rows": 1,
                          "Actual Loops": 4,
                          "Filter": "((qualification_profile = 'lng_v1'::text) AND (id = o.product_id) AND EXISTS(SubPlan 19))",
                          "Rows Removed by Filter": 0,
                          "Shared Hit Blocks": 0,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0,
                          "Plans": [
                            {
                              "Node Type": "Function Scan",
                              "Parent Relationship": "SubPlan",
                              "Subplan Name": "SubPlan 19",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Function Name": "jsonb_array_elements_text",
                              "Alias": "lane_1",
                              "Startup Cost": 0.0,
                              "Total Cost": 1.75,
                              "Plan Rows": 1,
                              "Plan Width": 0,
                              "Actual Startup Time": 0.005,
                              "Actual Total Time": 0.005,
                              "Actual Rows": 1,
                              "Actual Loops": 4,
                              "Filter": "(value = (o.delivery_point_id)::text)",
                              "Rows Removed by Filter": 0,
                              "Shared Hit Blocks": 0,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            }
                          ]
                        },
                        {
                          "Node Type": "CTE Scan",
                          "Parent Relationship": "SubPlan",
                          "Subplan Name": "SubPlan 14",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "CTE Name": "policy_products",
                          "Alias": "pf",
                          "Startup Cost": 0.0,
                          "Total Cost": 2.25,
                          "Plan Rows": 1,
                          "Plan Width": 16,
                          "Actual Startup Time": 0.002,
                          "Actual Total Time": 0.004,
                          "Actual Rows": 1,
                          "Actual Loops": 1,
                          "Filter": "(qualification_profile = 'fame_v1'::text)",
                          "Rows Removed by Filter": 5,
                          "Shared Hit Blocks": 0,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0
                        },
                        {
                          "Node Type": "CTE Scan",
                          "Parent Relationship": "SubPlan",
                          "Subplan Name": "SubPlan 16",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "CTE Name": "policy_products",
                          "Alias": "pf_1",
                          "Startup Cost": 0.0,
                          "Total Cost": 177.75,
                          "Plan Rows": 1,
                          "Plan Width": 0,
                          "Actual Startup Time": 0.008,
                          "Actual Total Time": 0.009,
                          "Actual Rows": 1,
                          "Actual Loops": 4,
                          "Filter": "((qualification_profile = 'fame_v1'::text) AND (id = o.product_id) AND EXISTS(SubPlan 15))",
                          "Rows Removed by Filter": 1,
                          "Shared Hit Blocks": 0,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0,
                          "Plans": [
                            {
                              "Node Type": "Function Scan",
                              "Parent Relationship": "SubPlan",
                              "Subplan Name": "SubPlan 15",
                              "Parallel Aware": false,
                              "Async Capable": false,
                              "Function Name": "jsonb_array_elements_text",
                              "Alias": "lane",
                              "Startup Cost": 0.0,
                              "Total Cost": 1.75,
                              "Plan Rows": 1,
                              "Plan Width": 0,
                              "Actual Startup Time": 0.005,
                              "Actual Total Time": 0.005,
                              "Actual Rows": 1,
                              "Actual Loops": 4,
                              "Filter": "(value = (o.delivery_point_id)::text)",
                              "Rows Removed by Filter": 0,
                              "Shared Hit Blocks": 0,
                              "Shared Read Blocks": 0,
                              "Shared Dirtied Blocks": 0,
                              "Shared Written Blocks": 0,
                              "Local Hit Blocks": 0,
                              "Local Read Blocks": 0,
                              "Local Dirtied Blocks": 0,
                              "Local Written Blocks": 0,
                              "Temp Read Blocks": 0,
                              "Temp Written Blocks": 0
                            }
                          ]
                        }
                      ]
                    },
                    {
                      "Node Type": "Materialize",
                      "Parent Relationship": "Inner",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Startup Cost": 0.0,
                      "Total Cost": 1.14,
                      "Plan Rows": 6,
                      "Plan Width": 40,
                      "Actual Startup Time": 0.0,
                      "Actual Total Time": 0.0,
                      "Actual Rows": 2,
                      "Actual Loops": 10051,
                      "Shared Hit Blocks": 1,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0,
                      "Plans": [
                        {
                          "Node Type": "Seq Scan",
                          "Parent Relationship": "Outer",
                          "Parallel Aware": false,
                          "Async Capable": false,
                          "Relation Name": "products",
                          "Alias": "p",
                          "Startup Cost": 0.0,
                          "Total Cost": 1.1,
                          "Plan Rows": 6,
                          "Plan Width": 40,
                          "Actual Startup Time": 0.048,
                          "Actual Total Time": 0.053,
                          "Actual Rows": 6,
                          "Actual Loops": 1,
                          "Filter": "(is_active AND ((fuel_type)::text = ANY ('{Methanol,Ethanol,FAME,LNG}'::text[])))",
                          "Rows Removed by Filter": 1,
                          "Shared Hit Blocks": 1,
                          "Shared Read Blocks": 0,
                          "Shared Dirtied Blocks": 0,
                          "Shared Written Blocks": 0,
                          "Local Hit Blocks": 0,
                          "Local Read Blocks": 0,
                          "Local Dirtied Blocks": 0,
                          "Local Written Blocks": 0,
                          "Temp Read Blocks": 0,
                          "Temp Written Blocks": 0
                        }
                      ]
                    }
                  ]
                },
                {
                  "Node Type": "Materialize",
                  "Parent Relationship": "Inner",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Startup Cost": 0.0,
                  "Total Cost": 1.13,
                  "Plan Rows": 8,
                  "Plan Width": 31,
                  "Actual Startup Time": 0.0,
                  "Actual Total Time": 0.0,
                  "Actual Rows": 1,
                  "Actual Loops": 10050,
                  "Shared Hit Blocks": 1,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0,
                  "Plans": [
                    {
                      "Node Type": "Seq Scan",
                      "Parent Relationship": "Outer",
                      "Parallel Aware": false,
                      "Async Capable": false,
                      "Relation Name": "delivery_points",
                      "Alias": "dp",
                      "Startup Cost": 0.0,
                      "Total Cost": 1.09,
                      "Plan Rows": 8,
                      "Plan Width": 31,
                      "Actual Startup Time": 0.012,
                      "Actual Total Time": 0.014,
                      "Actual Rows": 8,
                      "Actual Loops": 1,
                      "Filter": "is_active",
                      "Rows Removed by Filter": 1,
                      "Shared Hit Blocks": 1,
                      "Shared Read Blocks": 0,
                      "Shared Dirtied Blocks": 0,
                      "Shared Written Blocks": 0,
                      "Local Hit Blocks": 0,
                      "Local Read Blocks": 0,
                      "Local Dirtied Blocks": 0,
                      "Local Written Blocks": 0,
                      "Temp Read Blocks": 0,
                      "Temp Written Blocks": 0
                    }
                  ]
                }
              ]
            },
            {
              "Node Type": "CTE Scan",
              "Parent Relationship": "Inner",
              "Parallel Aware": false,
              "Async Capable": false,
              "CTE Name": "policy_products",
              "Alias": "pp",
              "Startup Cost": 0.0,
              "Total Cost": 2.25,
              "Plan Rows": 1,
              "Plan Width": 144,
              "Actual Startup Time": 0.001,
              "Actual Total Time": 0.001,
              "Actual Rows": 3,
              "Actual Loops": 10049,
              "Filter": "(execution_mode = 'ORDERBOOK'::text)",
              "Rows Removed by Filter": 0,
              "Shared Hit Blocks": 0,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0
            },
            {
              "Node Type": "Function Scan",
              "Parent Relationship": "SubPlan",
              "Subplan Name": "SubPlan 21",
              "Parallel Aware": false,
              "Async Capable": false,
              "Function Name": "jsonb_array_elements_text",
              "Alias": "lane_2",
              "Startup Cost": 0.0,
              "Total Cost": 1.75,
              "Plan Rows": 1,
              "Plan Width": 0,
              "Actual Startup Time": 0.01,
              "Actual Total Time": 0.01,
              "Actual Rows": 1,
              "Actual Loops": 10048,
              "Filter": "(value = (o.delivery_point_id)::text)",
              "Rows Removed by Filter": 3,
              "Shared Hit Blocks": 0,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0
            }
          ]
        },
        {
          "Node Type": "Function Scan",
          "Parent Relationship": "Inner",
          "Parallel Aware": false,
          "Async Capable": false,
          "Function Name": "jsonb_to_recordset",
          "Alias": "policy_point",
          "Startup Cost": 0.0,
          "Total Cost": 1.0,
          "Plan Rows": 100,
          "Plan Width": 80,
          "Actual Startup Time": 0.0,
          "Actual Total Time": 0.0,
          "Actual Rows": 4,
          "Actual Loops": 10047,
          "Shared Hit Blocks": 0,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0
        },
        {
          "Node Type": "CTE Scan",
          "Parent Relationship": "SubPlan",
          "Subplan Name": "SubPlan 3",
          "Parallel Aware": false,
          "Async Capable": false,
          "CTE Name": "policy_products",
          "Alias": "pp_1",
          "Startup Cost": 0.0,
          "Total Cost": 2.25,
          "Plan Rows": 1,
          "Plan Width": 32,
          "Actual Startup Time": 0.001,
          "Actual Total Time": 0.002,
          "Actual Rows": 1,
          "Actual Loops": 10047,
          "Filter": "(id = p.id)",
          "Rows Removed by Filter": 5,
          "Shared Hit Blocks": 0,
          "Shared Read Blocks": 0,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0
        }
      ]
    },
    {
      "Node Type": "CTE Scan",
      "Parent Relationship": "Inner",
      "Parallel Aware": false,
      "Async Capable": false,
      "CTE Name": "requested_keys",
      "Alias": "requested",
      "Startup Cost": 0.0,
      "Total Cost": 2.0,
      "Plan Rows": 100,
      "Plan Width": 112,
      "Actual Startup Time": 0.0,
      "Actual Total Time": 0.001,
      "Actual Rows": 1,
      "Actual Loops": 10047,
      "Shared Hit Blocks": 0,
      "Shared Read Blocks": 0,
      "Shared Dirtied Blocks": 0,
      "Shared Written Blocks": 0,
      "Local Hit Blocks": 0,
      "Local Read Blocks": 0,
      "Local Dirtied Blocks": 0,
      "Local Written Blocks": 0,
      "Temp Read Blocks": 0,
      "Temp Written Blocks": 0
    }
  ]
}
```
