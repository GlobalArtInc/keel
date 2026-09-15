CREATE TABLE alpha_thing (
    id       TEXT PRIMARY KEY,
    thing_id TEXT NOT NULL REFERENCES beta_thing(id),
    note     TEXT NOT NULL
);
