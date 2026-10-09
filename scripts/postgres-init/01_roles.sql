-- scripts/postgres-init/01_roles.sql
-- Run once when the Postgres container initialises.
--
-- Creates a restricted application role:
--   - `ladywalk_app`: can read and insert, and can update only the rows whose
--     lifecycle genuinely changes. ledger_entries grants no UPDATE or DELETE at
--     all, so append-only (Q03) does not depend on the application behaving.
--
-- The ledger's own triggers reject UPDATE and DELETE as well. Two mechanisms
-- rather than one is deliberate: the grants are what a compromised application
-- runs into, and the triggers are what a hand-written statement during an
-- incident runs into. Either alone leaves a way in.

-- Create the application role (idempotent)
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'ladywalk_app') THEN
        CREATE ROLE ladywalk_app LOGIN PASSWORD 'ladywalk_app_dev';
    END IF;
END
$$;

-- Grant schema access
GRANT CONNECT ON DATABASE ladywalk TO ladywalk_app;
GRANT USAGE   ON SCHEMA public     TO ladywalk_app;

-- Table-level grants.
--
-- These are kept here as the statement of intent, and re-applied by the migrate
-- service after each migration run, because a GRANT naming a table that does
-- not exist yet fails. Revoking by default privileges as well means a table
-- added by a later migration is *not* writable until it is granted here, so
-- the default for anything new is closed.
ALTER DEFAULT PRIVILEGES IN SCHEMA public REVOKE ALL ON TABLES FROM ladywalk_app;

-- Financial records: readable and appendable, never mutable.
GRANT SELECT, INSERT ON TABLE ledger_entries TO ladywalk_app;

-- Reference and lifecycle tables: orders and markets do change, so they are
-- updatable. The append-only rule is about money, not about state.
GRANT SELECT, INSERT, UPDATE ON TABLE accounts  TO ladywalk_app;
GRANT SELECT, INSERT, UPDATE ON TABLE markets  TO ladywalk_app;
GRANT SELECT, INSERT, UPDATE ON TABLE orders   TO ladywalk_app;
GRANT SELECT, INSERT         ON TABLE currencies TO ladywalk_app;

-- Quotes and idempotency records are written once and read back verbatim; a
-- replay returns the stored response rather than recomputing it, so allowing an
-- update would let a replay answer differently from the original.
GRANT SELECT, INSERT ON TABLE quotes           TO ladywalk_app;
GRANT SELECT, INSERT ON TABLE idempotency_keys TO ladywalk_app;
