-- Creates a table, then fails. MySQL commits the CREATE TABLE, so gizmos stays.
CREATE TABLE gizmos (
    id BIGINT NOT NULL PRIMARY KEY
);
INSERT INTO no_such_table VALUES (1);
