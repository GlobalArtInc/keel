CREATE TABLE broken_thing (
    id TEXT PRIMARY KEY,
    thing_id TEXT NOT NULL REFERENCES nowhere_thing(id)
);
