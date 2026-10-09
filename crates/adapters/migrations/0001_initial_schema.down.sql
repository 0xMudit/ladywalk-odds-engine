-- Migration: 0001_initial_schema (DOWN)
-- Reversal order is the reverse of creation order. Tables that reference each
-- other go first, and the ledger's triggers and functions are dropped with
-- their table.
--
-- DESTRUCTIVE: this deletes all ledger, order, quote, and account data.
-- Only run this in local and staging environments.

DROP TABLE IF EXISTS ledger_entries;
DROP TABLE IF EXISTS idempotency_keys;
DROP TABLE IF EXISTS quotes;
DROP TABLE IF EXISTS order_matches;
DROP TABLE IF EXISTS orders;
DROP TABLE IF EXISTS markets;
DROP TABLE IF EXISTS accounts;
DROP TABLE IF EXISTS currencies;

-- Dropped explicitly as well as with their tables, so that a failed rollback
-- does not leave a function behind for the next migration to collide with.
DROP FUNCTION IF EXISTS assert_ledger_transaction_is_well_formed();
DROP FUNCTION IF EXISTS ledger_entries_are_append_only();
DROP FUNCTION IF EXISTS order_matches_share_a_market();
DROP FUNCTION IF EXISTS create_market_holding_account();

DROP EXTENSION IF EXISTS "pgcrypto";
