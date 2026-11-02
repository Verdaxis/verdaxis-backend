CREATE TABLE market_command_results (
 id uuid PRIMARY KEY, actor_user_id uuid NOT NULL, effective_organization_id uuid NOT NULL,
 support_context_id uuid, operation text NOT NULL, idempotency_key text NOT NULL,
 request_hash text NOT NULL, resource_type text NOT NULL, resource_id uuid NOT NULL,
 response_status smallint NOT NULL, response_body jsonb,
 CONSTRAINT uq_market_command_results_actor_operation_key UNIQUE(actor_user_id,operation,idempotency_key)
);
CREATE TABLE market_event_outbox (
 id uuid PRIMARY KEY, event_type text, aggregate_type text, aggregate_id text,
 participant_org_ids json, payload json
);
CREATE TABLE orderbook_orders (
 id uuid PRIMARY KEY, organization_id uuid NOT NULL, owner_user_id uuid,
 created_by_actor_user_id uuid, idempotency_operation text, idempotency_key text,
 idempotency_request_hash text,
 UNIQUE(organization_id,idempotency_operation,idempotency_key)
);
