-- SQLITE TIMINGS DATABASE SCHEMA

CREATE TABLE IF NOT EXISTS client (
    id   INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    name TEXT NOT NULL,
    CONSTRAINT UQ_CLIENT_NAME UNIQUE (name)
) STRICT;

CREATE TABLE IF NOT EXISTS project (
    id       INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    name     TEXT NOT NULL,
    clientId INTEGER NOT NULL,
    CONSTRAINT UQ_CLIENT_PROJECT_NAME UNIQUE (name, clientId),
    CONSTRAINT FK_PROJECT_CLIENT_ID FOREIGN KEY (clientId)
    REFERENCES client (id) ON DELETE NO ACTION
                            ON UPDATE NO ACTION
) STRICT;

CREATE TABLE IF NOT EXISTS summary (
    id        INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    archived  INT NOT NULL, -- BOOLEAN
    start     INTEGER NOT NULL, -- Unix timestamp in milliseconds
    [end]     INTEGER NOT NULL, -- Unix timestamp in milliseconds
    text      TEXT NOT NULL,
    projectId INTEGER NOT NULL,
    CONSTRAINT UQ_CLIENT_PROJECT_NAME UNIQUE (projectId, start, [end]),
    CONSTRAINT FK_SUMMARY_PROJECT_ID FOREIGN KEY (projectId)
    REFERENCES project (id) ON DELETE NO ACTION
                            ON UPDATE NO ACTION
) STRICT;

CREATE INDEX IF NOT EXISTS IDX_SUMMARY_START ON summary (start);

CREATE TABLE IF NOT EXISTS timing (
    id        INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    start     INTEGER NOT NULL, -- Unix timestamp in milliseconds
    [end]     INTEGER NOT NULL, -- Unix timestamp in milliseconds
    projectId INTEGER NOT NULL,
    CONSTRAINT UQ_CLIENT_PROJECT_NAME UNIQUE (projectId, start),
    CONSTRAINT FK_TIMING_PROJECT_ID FOREIGN KEY (projectId)
    REFERENCES project (id) ON DELETE NO ACTION
                            ON UPDATE NO ACTION
) STRICT;

CREATE INDEX IF NOT EXISTS IDX_TIMING_START ON timing (start);


-- This view performs poorly, but it's a helper for manual queries

CREATE VIEW IF NOT EXISTS dailyTotals AS
    SELECT strftime('%Y-%m-%d', CAST (start AS REAL) / 1000, 'unixepoch', 'localtime') AS day,
        CAST (SUM([end] - start) AS REAL) / 3600000 AS hours,
        client.name AS client,
        project.name AS project,
        projectId
    FROM timing,
        project,
        client
    WHERE 1=1
        AND timing.projectId = project.id
        AND project.clientId = client.id
    GROUP BY projectId, day 
    ORDER BY start DESC;


-- This view performs poorly, but it's a helper for manual queries, not to be used by application code

CREATE VIEW IF NOT EXISTS dailySummaries AS
    SELECT 
        strftime('%Y-%m-%d', CAST (s.start AS REAL) / 1000, 'unixepoch', 'localtime') AS day,
        s.text as summary, 
        c.name as client, 
        p.name as project,
        s.projectId
    FROM 
        summary as s, 
        client as c, 
        project as p 
    WHERE 1=1
        AND p.id = s.projectId
        AND p.clientId = c.id
    ORDER BY s.start DESC;

