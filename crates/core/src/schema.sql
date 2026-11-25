CREATE TABLE IF NOT EXISTS transfers(
 id INTEGER PRIMARY KEY, root INTEGER, time INTEGER NOT NULL, direction TEXT NOT NULL,
 partner TEXT NOT NULL, metadata TEXT NOT NULL, bytes BLOB NOT NULL, status TEXT NOT NULL,
 execution INTEGER, resend_of INTEGER, claim TEXT UNIQUE
);
CREATE TABLE IF NOT EXISTS interchanges(
 id INTEGER PRIMARY KEY, transfer INTEGER NOT NULL, sender TEXT NOT NULL, recipient TEXT NOT NULL,
 reference TEXT NOT NULL, test_flag TEXT NOT NULL, ack_requested TEXT NOT NULL, ack_status TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS messages(
 id INTEGER PRIMARY KEY, interchange INTEGER NOT NULL, type TEXT NOT NULL, version TEXT NOT NULL,
 release TEXT NOT NULL, reference TEXT NOT NULL, segment_count INTEGER NOT NULL,
 grp TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS findings(
 id INTEGER PRIMARY KEY, transfer INTEGER NOT NULL, interchange INTEGER, severity TEXT NOT NULL,
 code TEXT NOT NULL, offset INTEGER NOT NULL, text TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS executions(
 id INTEGER PRIMARY KEY, root INTEGER, interchange INTEGER, message INTEGER, partner TEXT NOT NULL, name TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS events(id INTEGER PRIMARY KEY, body TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS partner_state(partner TEXT PRIMARY KEY, reference INTEGER NOT NULL, data TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS transfer_root ON transfers(root);
CREATE INDEX IF NOT EXISTS interchange_ref ON interchanges(reference);

-- Transfer IDs also name published files. Keep their high-water mark across retention.
CREATE TABLE IF NOT EXISTS transfer_sequence(singleton INTEGER PRIMARY KEY, last_id INTEGER NOT NULL);
INSERT OR IGNORE INTO transfer_sequence SELECT 1, COALESCE(MAX(id),0) FROM transfers;
