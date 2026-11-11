//! Disposable PostgreSQL cases, selected by run_agent_tools.sh.
#![cfg(feature = "contract-tests")]
use serde_json::{Value, json};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use uuid::Uuid;
use verdaxis_read::{
    AppState,
    agent::{audit::verify_chain, tools::*},
};

struct Fixture {
    app: PgPool,
    admin: PgPool,
    state: AppState,
    user: Uuid,
    member: Uuid,
    foreign_user: Uuid,
    organization: Uuid,
    conversation: Uuid,
    foreign_conversation: Uuid,
    product: String,
    point: String,
    order: Uuid,
}
impl Fixture {
    async fn new() -> Self {
        let app_url = std::env::var("AGENT_TOOLS_APP_DATABASE_URL").expect("disposable app URL");
        let admin_url =
            std::env::var("AGENT_TOOLS_ADMIN_DATABASE_URL").expect("disposable admin URL");
        for url in [&app_url, &admin_url] {
            let url = url::Url::parse(url).unwrap();
            assert_eq!(url.host_str(), Some("127.0.0.1"));
            assert_eq!(url.path(), "/verdaxis_agent_tools_test");
        }
        let app = verdaxis_read::runtime::db::connect_contract_pool(
            app_url.parse().unwrap(),
            6,
            "UTC".into(),
        )
        .await
        .unwrap();
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
            .unwrap();
        sqlx::raw_sql("TRUNCATE public.agent_audit_events RESTART IDENTITY; TRUNCATE public.agent_conversations,public.order_drafts,public.agent_messages,public.organizations,public.users CASCADE;").execute(&admin).await.unwrap();
        sqlx::raw_sql(include_str!("fixtures/phase8_effects.sql"))
            .execute(&admin)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("fixtures/phase8_cancel.sql"))
            .execute(&admin)
            .await
            .unwrap();
        let policy: verdaxis_read::MarketPolicy =
            serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let product = policy
            .products
            .iter()
            .find(|p| p.qualification_profile == "scalar" && p.execution_mode == "ORDERBOOK")
            .unwrap();
        let catalog: Value =
            serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let product_json = catalog["products"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == product.id.to_string())
            .unwrap();
        let point_id: Uuid = product_json["lanes"][0].as_str().unwrap().parse().unwrap();
        let point = policy
            .points
            .iter()
            .find(|point| point.id == point_id)
            .unwrap();
        let order = Uuid::new_v4();
        let user = Uuid::new_v4();
        let member = Uuid::new_v4();
        let foreign_user = Uuid::new_v4();
        let organization = Uuid::new_v4();
        let support = Uuid::new_v4();
        sqlx::query("SELECT cancel_fixture_seed($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
            .bind(order)
            .bind(user)
            .bind(member)
            .bind(foreign_user)
            .bind(organization)
            .bind(support)
            .bind(product.id)
            .bind(point_id)
            .bind(&product.code)
            .bind(
                product_json["specification_standard"]
                    .as_str()
                    .unwrap_or("ISO 8217"),
            )
            .execute(&admin)
            .await
            .unwrap();
        let product_code = product.code.clone();
        let point_name = point.name.clone();
        let conversation = Uuid::new_v4();
        let foreign_conversation = Uuid::new_v4();
        sqlx::query("INSERT INTO public.agent_conversations(id,user_id,organization_id) VALUES($1,$2,$3),($4,$5,$3)")
            .bind(conversation).bind(user).bind(organization).bind(foreign_conversation).bind(member).execute(&admin).await.unwrap();
        for (conversation, seq, role, text) in [
            (conversation, 1, "user", "Please prepare ten tonnes."),
            (conversation, 2, "assistant", "Assistant text"),
            (foreign_conversation, 3, "user", "Other person's request"),
        ] {
            sqlx::query("INSERT INTO public.agent_messages(id,conversation_id,seq,role,content) VALUES($1,$2,$3,$4,$5)").bind(Uuid::new_v4()).bind(conversation).bind(seq).bind(role).bind(json!(text)).execute(&admin).await.unwrap();
        }
        let config=serde_json::from_value(json!({"environment":"test","release_sha":verdaxis_read::compiled_sha(),"migration_revision":verdaxis_read::runtime::artifact::ALLOWED_REVISIONS[0],"database_url":app_url,"db_session_time_zone":"UTC","bind":"127.0.0.1:0","read_admission_slots":6,"pool_max":7,"python_budget":0,"configured_max_connections":100})).unwrap();
        let state = AppState::new(
            config,
            verdaxis_read::runtime::db::Pools {
                read: app.clone(),
                health: verdaxis_read::runtime::db::connect_contract_pool(
                    app_url.parse().unwrap(),
                    1,
                    "UTC".into(),
                )
                .await
                .unwrap(),
            },
            policy,
        )
        .unwrap();
        Self {
            app,
            admin,
            state,
            user,
            member,
            foreign_user,
            organization,
            conversation,
            foreign_conversation,
            product: product_code,
            point: point_name,
            order,
        }
    }
    fn context(&self) -> ToolContext {
        ToolContext {
            state: self.state.clone(),
            user_id: self.user,
            organization_id: Some(self.organization),
            conversation_id: self.conversation,
        }
    }
    fn draft(&self, seq: i32) -> AgentTool {
        serde_json::from_value(json!({"tool":"create_draft","arguments":{"side":"BID","market_product":self.product,"delivery_point":self.point,"availability_window":"SPOT","quantity_mt":"10.00","limit_price_usd":"100.00","source_message_seq":seq}})).unwrap()
    }
    async fn count(&self, table: &str) -> i64 {
        match table {
            "orders" => sqlx::query_scalar("SELECT count(*) FROM public.orderbook_orders")
                .fetch_one(&self.admin)
                .await
                .unwrap(),
            "drafts" => sqlx::query_scalar("SELECT count(*) FROM public.order_drafts")
                .fetch_one(&self.admin)
                .await
                .unwrap(),
            _ => panic!("unsupported table"),
        }
    }
}
fn call(name: &str, args: Value) -> AgentTool {
    serde_json::from_value(json!({"tool":name,"arguments":args})).unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL runner"]
async fn create_draft_uses_read_write_transactions_on_read_only_sessions() {
    let f = Fixture::new().await;
    let mut tx = f.app.begin().await.unwrap();
    let read_only: String = sqlx::query_scalar("SHOW transaction_read_only")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(read_only, "on");
    let error = sqlx::query("INSERT INTO public.agent_audit_events(kind,payload,prev_hash,hash) VALUES('draft_created','{}',$1,$1)")
        .bind(verdaxis_read::agent::audit::GENESIS_HASH)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("25006")
    );
    tx.rollback().await.unwrap();

    let orders = f.count("orders").await;
    let result = execute(&f.context(), f.draft(1)).await;
    assert!(result.ok, "{:?}", result.content);
    assert_eq!(f.count("drafts").await, 1);
    assert_eq!(f.count("orders").await, orders);
    let kinds: Vec<String> =
        sqlx::query_scalar("SELECT kind FROM public.agent_audit_events ORDER BY id")
            .fetch_all(&f.app)
            .await
            .unwrap();
    assert_eq!(
        kinds,
        ["tool_call_intent", "draft_created", "tool_call_receipt"]
    );
    let report = verify_chain(&f.app, None, None).await.unwrap();
    assert_eq!(report.checked, 3);
    assert_eq!(report.first_broken_id, None);
    let read_only: String = sqlx::query_scalar("SHOW default_transaction_read_only")
        .fetch_one(&f.app)
        .await
        .unwrap();
    assert_eq!(read_only, "on");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL runner"]
async fn draft_and_event_commit_without_an_order() {
    let f = Fixture::new().await;
    let before = f.count("orders").await;
    let result = execute(&f.context(), f.draft(1)).await;
    assert!(result.ok, "{:?}", result.content);
    assert_eq!(result.provenance, ToolProvenance::UserData);
    assert_eq!(f.count("orders").await, before);
    assert_eq!(f.count("drafts").await, 1);
    let row=sqlx::query("SELECT status,source_text,terms_digest,quantity_mt::text AS quantity FROM public.order_drafts").fetch_one(&f.admin).await.unwrap();
    assert_eq!(row.get::<String, _>("status"), "draft");
    assert_eq!(
        row.get::<String, _>("source_text"),
        "Please prepare ten tonnes."
    );
    assert_eq!(row.get::<String, _>("quantity"), "10");
    assert_eq!(
        result.content["terms_digest"],
        row.get::<String, _>("terms_digest")
    );
    let kinds: Vec<String> =
        sqlx::query_scalar("SELECT kind FROM public.agent_audit_events ORDER BY id")
            .fetch_all(&f.app)
            .await
            .unwrap();
    assert_eq!(
        kinds,
        ["tool_call_intent", "draft_created", "tool_call_receipt"]
    );
    assert_eq!(verify_chain(&f.app, None, None).await.unwrap().checked, 3);
}
#[tokio::test]
#[ignore = "requires disposable PostgreSQL runner"]
async fn source_role_and_conversation_are_required() {
    let f = Fixture::new().await;
    for seq in [2, 3, 999] {
        let result = execute(&f.context(), f.draft(seq)).await;
        assert!(!result.ok);
        assert_eq!(result.content["error_code"], "INVALID_ARGUMENTS");
    }
    assert_eq!(f.count("drafts").await, 0);
    let receipts:i64=sqlx::query_scalar("SELECT count(*) FROM public.agent_audit_events WHERE kind='tool_call_receipt' AND payload->'outcome'->>'ok'='false'").fetch_one(&f.app).await.unwrap();
    assert_eq!(receipts, 3);
    sqlx::query("UPDATE public.agent_conversations SET status='closed' WHERE id=$1")
        .bind(f.conversation)
        .execute(&f.admin)
        .await
        .unwrap();
    assert!(!execute(&f.context(), f.draft(1)).await.ok);
    sqlx::query("UPDATE public.agent_conversations SET status='open' WHERE id=$1")
        .bind(f.conversation)
        .execute(&f.admin)
        .await
        .unwrap();
    sqlx::query("UPDATE public.users SET organization_id=NULL WHERE id=$1")
        .bind(f.user)
        .execute(&f.admin)
        .await
        .unwrap();
    assert!(!execute(&f.context(), f.draft(1)).await.ok);
    assert_eq!(f.count("drafts").await, 0);
}
#[tokio::test]
#[ignore = "requires disposable PostgreSQL runner"]
async fn two_users_cannot_read_or_create_each_others_private_data() {
    let f = Fixture::new().await;
    assert!(execute(&f.context(), f.draft(1)).await.ok);
    let orders = execute(&f.context(), call("get_my_orders", json!({}))).await;
    assert!(orders.ok, "{:?}", orders.content);
    assert_eq!(orders.content[0]["id"], f.order.to_string());
    let trades = execute(&f.context(), call("get_my_trades", json!({}))).await;
    assert!(trades.ok, "{:?}", trades.content);
    assert_eq!(trades.content["items"].as_array().unwrap().len(), 1);
    let member = ToolContext {
        state: f.state.clone(),
        user_id: f.member,
        organization_id: Some(f.organization),
        conversation_id: f.foreign_conversation,
    };
    let orders = execute(&member, call("get_my_orders", json!({}))).await;
    assert!(orders.ok);
    assert_eq!(orders.content, json!([]));
    let trades = execute(&member, call("get_my_trades", json!({}))).await;
    assert!(trades.ok);
    assert_eq!(trades.content["items"], json!([]));
    let forged = ToolContext {
        state: f.state.clone(),
        user_id: f.member,
        organization_id: Some(f.organization),
        conversation_id: f.conversation,
    };
    assert!(!execute(&forged, f.draft(1)).await.ok);
    assert!(!execute(&forged, call("get_my_orders", json!({}))).await.ok);
    let other_org: Uuid =
        sqlx::query_scalar("SELECT organization_id FROM public.users WHERE id=$1")
            .bind(f.foreign_user)
            .fetch_one(&f.admin)
            .await
            .unwrap();
    let foreign = ToolContext {
        state: f.state.clone(),
        user_id: f.foreign_user,
        organization_id: Some(other_org),
        conversation_id: f.conversation,
    };
    assert!(!execute(&foreign, f.draft(1)).await.ok);
    let owners: Vec<Uuid> = sqlx::query_scalar("SELECT user_id FROM public.order_drafts")
        .fetch_all(&f.admin)
        .await
        .unwrap();
    assert_eq!(owners, [f.user]);
}
#[tokio::test]
#[ignore = "requires disposable PostgreSQL runner"]
async fn every_tool_and_failed_execution_have_digest_only_receipts() {
    let f = Fixture::new().await;
    let request = json!({"vessel":{"speed_knots":"14","sea_consumption_mt_per_day":"20","compatible_fuels":["VLSFO"]},"legs":[{"from_port":"Singapore","to_port":"Rotterdam"}],"start_date":chrono::Utc::now().date_naive().to_string()});
    let tools = vec![
        call("list_markets", json!({})),
        call(
            "get_market_slice",
            json!({"market_product":f.product,"delivery_point":f.point,"availability_window":"SPOT"}),
        ),
        call("plan_voyage_fuel", json!({"request":request})),
        call("get_my_orders", json!({})),
        call("get_my_trades", json!({})),
        f.draft(1),
        call("ask_user", json!({"question":"Which port?"})),
        call("get_my_trades", json!({"limit":0})),
    ];
    let mut results = Vec::new();
    for tool in tools {
        results.push(execute(&f.context(), tool).await);
    }
    for result in &results[..7] {
        assert!(result.ok, "{:?}", result.content);
        assert!(result.content.to_string().chars().count() <= MAX_CONTENT_CHARS);
    }
    assert!(!results[7].ok);
    let rows:Vec<(String,Value)>=sqlx::query_as("SELECT kind,payload FROM public.agent_audit_events WHERE kind IN ('tool_call_intent','tool_call_receipt') ORDER BY id").fetch_all(&f.app).await.unwrap();
    assert_eq!(rows.len(), 16);
    for (index, pair) in rows.chunks_exact(2).enumerate() {
        assert_eq!(pair[0].0, "tool_call_intent");
        assert!(pair[0].1["tool"].is_string());
        assert!(pair[0].1["arguments"].is_object());
        assert_eq!(pair[1].0, "tool_call_receipt");
        let outcome = &pair[1].1["outcome"];
        assert_eq!(outcome["ok"], results[index].ok);
        assert_eq!(outcome["result_digest"].as_str().unwrap().len(), 64);
        assert!(outcome.get("content").is_none());
        assert!(outcome["chars"].as_u64().unwrap() <= 12000);
        assert_eq!(
            outcome["provenance"],
            serde_json::to_value(results[index].provenance).unwrap()
        );
    }
}
#[tokio::test]
#[ignore = "requires disposable PostgreSQL runner"]
async fn draft_audit_failure_rolls_back_and_failed_receipt_is_not_success() {
    let f = Fixture::new().await;
    sqlx::raw_sql("CREATE FUNCTION agent_tools_refuse_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind='draft_created' THEN RAISE EXCEPTION 'private diagnostic'; END IF; RETURN NEW; END $$; CREATE TRIGGER agent_tools_refuse_event BEFORE INSERT ON public.agent_audit_events FOR EACH ROW EXECUTE FUNCTION agent_tools_refuse_event();").execute(&f.admin).await.unwrap();
    let result = execute(&f.context(), f.draft(1)).await;
    assert!(!result.ok);
    assert!(!result.content.to_string().contains("private diagnostic"));
    assert_eq!(f.count("drafts").await, 0);
    sqlx::raw_sql("DROP TRIGGER agent_tools_refuse_event ON public.agent_audit_events; DROP FUNCTION agent_tools_refuse_event(); CREATE FUNCTION agent_tools_refuse_receipt() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind='tool_call_receipt' THEN RAISE EXCEPTION 'private diagnostic'; END IF; RETURN NEW; END $$; CREATE TRIGGER agent_tools_refuse_receipt BEFORE INSERT ON public.agent_audit_events FOR EACH ROW EXECUTE FUNCTION agent_tools_refuse_receipt();").execute(&f.admin).await.unwrap();
    let result = execute(&f.context(), call("ask_user", json!({"question":"Port?"}))).await;
    assert!(!result.ok);
    assert_eq!(result.content["error_code"], "AUDIT_UNAVAILABLE");
    sqlx::raw_sql("DROP TRIGGER agent_tools_refuse_receipt ON public.agent_audit_events; DROP FUNCTION agent_tools_refuse_receipt();").execute(&f.admin).await.unwrap();
}
