-- Test fixture: one sequenced outbox event for one organization, so a
-- private trade stream that reconnects with a cursor has an event to replay.
-- $1 event id, $2 aggregate id, $3 participant organization id.
INSERT INTO market_event_outbox (id, event_type, aggregate_type, aggregate_id, participant_org_ids, payload, created_at, dispatched_at, stream_seq)
VALUES ($1, 'synthetic_replay', 'synthetic', $2, json_build_array($3::text), json_build_object('marker', $2::text), now(), now(), nextval('market_event_stream_seq'))
RETURNING stream_seq
