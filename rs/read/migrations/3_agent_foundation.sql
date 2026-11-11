-- Agent storage is independent of order placement. Runtime grants are owned by
-- deploy/postgres/app_acl_policy.sql and rebuilt after migration.
CREATE TABLE public.agent_conversations (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES public.users(id),
    organization_id uuid REFERENCES public.organizations(id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    status text NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'closed'))
);
CREATE INDEX agent_conversations_user_id_idx ON public.agent_conversations(user_id);
CREATE INDEX agent_conversations_organization_id_idx ON public.agent_conversations(organization_id);

CREATE TABLE public.agent_messages (
    id uuid PRIMARY KEY,
    conversation_id uuid NOT NULL REFERENCES public.agent_conversations(id),
    seq integer NOT NULL CHECK (seq > 0),
    role text NOT NULL CHECK (role IN ('user', 'assistant', 'tool', 'system_note')),
    content jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (conversation_id, seq)
);
CREATE INDEX agent_messages_user_window_idx ON public.agent_messages (conversation_id, created_at) WHERE role = 'user';

CREATE TABLE public.order_drafts (
    id uuid PRIMARY KEY,
    conversation_id uuid NOT NULL REFERENCES public.agent_conversations(id),
    user_id uuid NOT NULL REFERENCES public.users(id),
    side text NOT NULL CHECK (side IN ('BID', 'ASK')),
    market_product text NOT NULL,
    delivery_point text NOT NULL,
    availability_window text NOT NULL,
    quantity_mt numeric NOT NULL CHECK (quantity_mt > 0 AND quantity_mt < 'Infinity'::numeric),
    limit_price_usd numeric CHECK (limit_price_usd > 0 AND limit_price_usd < 'Infinity'::numeric),
    expires_at timestamptz,
    terms_digest text NOT NULL,
    source_text text NOT NULL,
    status text NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'confirmed', 'rejected', 'expired')),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX order_drafts_conversation_id_idx ON public.order_drafts(conversation_id);
CREATE INDEX order_drafts_user_id_idx ON public.order_drafts(user_id);

-- Actor identifiers are retained evidence, not cascading references. The app
-- can append and read events, but cannot rewrite or remove them.
CREATE TABLE public.agent_audit_events (
    id bigserial PRIMARY KEY,
    at timestamptz NOT NULL DEFAULT now(),
    user_id uuid,
    organization_id uuid,
    conversation_id uuid,
    kind text NOT NULL,
    payload jsonb NOT NULL,
    prev_hash text NOT NULL,
    hash text NOT NULL
);
CREATE INDEX agent_audit_events_at_idx ON public.agent_audit_events(at);
CREATE INDEX agent_audit_events_user_at_idx ON public.agent_audit_events (user_id, at) WHERE user_id IS NOT NULL;
CREATE INDEX agent_audit_events_organization_at_idx ON public.agent_audit_events (organization_id, at) WHERE organization_id IS NOT NULL;
CREATE INDEX agent_audit_events_conversation_id_idx ON public.agent_audit_events(conversation_id);

CREATE TABLE public.agent_controls (
    scope text NOT NULL CHECK (scope IN ('global', 'organization', 'user')),
    scope_id uuid,
    paused boolean NOT NULL DEFAULT false,
    reason text NOT NULL,
    updated_by uuid NOT NULL REFERENCES public.users(id),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK ((scope = 'global') = (scope_id IS NULL)),
    UNIQUE NULLS NOT DISTINCT (scope, scope_id)
);
