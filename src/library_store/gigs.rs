//! Gig rows are written inside the same SQLite transaction as the confirmed playback history.
use super::InspectError;
use crate::library::gigs::{
    valid_name, GigBook, GigDelta, GigRule, GigStatus, MAX_GIGS, MAX_RULES, NAME_BYTES,
    REASON_BYTES,
};
use rusqlite::{params, Connection};

pub(super) fn schema(c: &rusqlite::Connection) -> Result<(), String> {
    c.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS gigs (
            id INTEGER PRIMARY KEY CHECK (id > 0 AND id < 2147483647),
            name TEXT NOT NULL COLLATE NOCASE UNIQUE CHECK (length(CAST(name AS BLOB)) BETWEEN 1 AND 128)
        ) STRICT;

        CREATE TABLE IF NOT EXISTS gig_selection (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            active_gig_id INTEGER REFERENCES gigs(id) ON DELETE SET NULL
        ) STRICT;

        CREATE TABLE IF NOT EXISTS gig_rules (
            gig_id INTEGER NOT NULL REFERENCES gigs(id) ON DELETE CASCADE,
            track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
            status TEXT NOT NULL CHECK (status IN ('request', 'required', 'avoid')),
            reason TEXT NOT NULL DEFAULT '' CHECK (length(CAST(reason AS BLOB)) <= 512),
            fulfilled_at_ms INTEGER CHECK (fulfilled_at_ms IS NULL OR (fulfilled_at_ms >= 0 AND status='required')),
            PRIMARY KEY (gig_id, track_id)
        ) STRICT;

        INSERT OR IGNORE INTO gig_selection (singleton, active_gig_id) VALUES (1, NULL);
        "#,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
pub(super) fn read(c: &Connection) -> Result<GigBook, InspectError> {
    let mut book = GigBook::default();
    let mut names = c.prepare(
        "SELECT id, CASE WHEN length(CAST(name AS BLOB))<=?1 THEN name END FROM gigs LIMIT ?2",
    )?;
    let mut rows = names.query(params![NAME_BYTES as i64, MAX_GIGS as i64 + 1])?;
    while let Some(row) = rows.next()? {
        if book.names.len() == MAX_GIGS {
            return Err(InspectError::Damaged("Too many gigs".into()));
        }
        book.names.insert(row.get(0)?, row.get(1)?);
    }
    book.active = c.query_row(
        "SELECT active_gig_id FROM gig_selection WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let mut rules=c.prepare("SELECT gig_id,track_id,CASE WHEN length(CAST(status AS BLOB))<=8 THEN status END,
        CASE WHEN length(CAST(reason AS BLOB))<=?1 THEN reason END,fulfilled_at_ms FROM gig_rules LIMIT ?2")?;
    let mut rows = rules.query(params![REASON_BYTES as i64, MAX_RULES as i64 + 1])?;
    while let Some(row) = rows.next()? {
        if book.rules.len() == MAX_RULES {
            return Err(InspectError::Damaged("Too many gig rules".into()));
        }
        let raw: String = row.get(2)?;
        let status = GigStatus::parse(&raw)
            .ok_or_else(|| InspectError::Damaged("Unknown gig rule status".into()))?;
        book.rules.insert(
            (row.get(0)?, row.get(1)?),
            GigRule {
                status,
                reason: row.get(3)?,
                fulfilled_at_ms: row
                    .get::<_, Option<i64>>(4)?
                    .map(u64::try_from)
                    .transpose()
                    .map_err(|_| {
                        InspectError::Damaged("Negative gig fulfillment timestamp".into())
                    })?,
            },
        );
    }
    Ok(book)
}

pub(super) fn save_delta(c: &Connection, d: &GigDelta) -> Result<(), String> {
    if d.is_empty() {
        return Ok(());
    }
    write(
        c,
        d.names.iter().map(|(id, name)| (*id, name.as_str())),
        d.selection,
        d.rules.iter().map(|(pair, rule)| (*pair, rule.as_ref())),
    )
}

pub(super) fn write_full(c: &Connection, book: &GigBook) -> Result<(), String> {
    c.execute("DELETE FROM gigs", [])
        .map_err(|e| e.to_string())?;
    write(
        c,
        book.names.iter().map(|(&id, name)| (id, name.as_str())),
        Some(book.active),
        book.rules.iter().map(|(&pair, r)| (pair, Some(r))),
    )
}

fn write<'a>(
    c: &Connection,
    names: impl Iterator<Item = (i32, &'a str)>,
    selection: Option<Option<i32>>,
    rules: impl Iterator<Item = ((i32, i32), Option<&'a GigRule>)>,
) -> Result<(), String> {
    let err = |e: rusqlite::Error| e.to_string();
    let mut insert=c.prepare("INSERT INTO gigs(id,name) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET name=excluded.name WHERE name IS NOT excluded.name").map_err(err)?;
    for (id, name) in names {
        if !valid_name(name) {
            return Err("Invalid gig name".into());
        }
        insert.execute(params![id, name]).map_err(err)?;
    }
    if let Some(active) = selection {
        if c.execute(
            "UPDATE gig_selection SET active_gig_id=?1 WHERE singleton=1",
            [active],
        )
        .map_err(err)?
            != 1
        {
            return Err("Missing gig selection".into());
        }
    }
    let mut upsert=c.prepare("INSERT INTO gig_rules(gig_id,track_id,status,reason,fulfilled_at_ms) VALUES(?1,?2,?3,?4,?5)
        ON CONFLICT(gig_id,track_id) DO UPDATE SET status=excluded.status,reason=excluded.reason,fulfilled_at_ms=excluded.fulfilled_at_ms
        WHERE status IS NOT excluded.status OR reason IS NOT excluded.reason OR fulfilled_at_ms IS NOT excluded.fulfilled_at_ms").map_err(err)?;
    let mut remove = c
        .prepare("DELETE FROM gig_rules WHERE gig_id=?1 AND track_id=?2")
        .map_err(err)?;
    for ((gig, track), rule) in rules {
        if let Some(rule) = rule {
            if !rule.valid() {
                return Err("Invalid gig rule".into());
            }
            let fulfilled_at_ms = rule
                .fulfilled_at_ms
                .map(i64::try_from)
                .transpose()
                .map_err(|_| "Gig fulfillment timestamp exceeds SQLite range")?;
            upsert
                .execute(params![
                    gig,
                    track,
                    rule.status.as_str(),
                    rule.reason,
                    fulfilled_at_ms
                ])
                .map_err(err)?;
        } else {
            remove.execute(params![gig, track]).map_err(err)?;
        }
    }
    let (gig_count, rule_count): (i64, i64) = c
        .query_row(
            "SELECT (SELECT count(*) FROM gigs), (SELECT count(*) FROM gig_rules)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(err)?;
    if gig_count > MAX_GIGS as i64 || rule_count > MAX_RULES as i64 {
        return Err("Gig or rule limit exceeded".into());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_store::{inspect, Store, SCHEMA_VERSION};
    use crate::test_support::fixture;

    #[test]
    fn gig_rules_upgrade_v20_reopen_and_commit_fulfillment_with_play_history() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("library.sqlite3");
        let (mut store, mut library, _) = Store::open(path.clone()).unwrap();
        library.insert([fixture()]);
        library.record_play(1, 5, 0).unwrap();
        store.save_delta(&library.changes()).unwrap();
        store.connection.execute_batch("DROP TABLE gig_rules; DROP TABLE gig_selection; DROP TABLE gigs; PRAGMA user_version=20;").unwrap();
        drop(store);
        let (mut store, mut library, _) = Store::open(path.clone()).unwrap();
        assert_eq!(
            store
                .connection
                .pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        assert_eq!(library.gigs().count(), 0);
        assert_eq!(library.active_gig(), None);
        let a = library.create_gig("Gig A").unwrap();
        library
            .set_gig_rule(a, 1, Some(GigStatus::Avoid), "Opening hour")
            .unwrap();
        let b = library.create_gig("Gig B").unwrap();
        library
            .set_gig_rule(b, 1, Some(GigStatus::Required), "Birthday")
            .unwrap();
        store.save_delta(&library.changes()).unwrap();
        drop(store);
        let (mut store, mut library, _) = Store::open(path.clone()).unwrap();
        assert_eq!(library.active_gig(), Some(b));
        assert_eq!(library.gig_rule(a, 1).unwrap().status, GigStatus::Avoid);
        assert_eq!(library.gig_rule(b, 1).unwrap().fulfilled_at_ms, None);
        // Force the second half of the transaction to fail. History must still show the pre-gig entry.
        store.connection.execute_batch("CREATE TEMP TRIGGER refuse_rule BEFORE UPDATE ON gig_rules BEGIN SELECT RAISE(ABORT,'test failure'); END;").unwrap();
        library.record_play(1, 25, 1).unwrap();
        let delta = library.changes();
        assert!(store.save_delta(&delta).is_err());
        let failed = inspect(&store.connection).unwrap().unwrap();
        assert_eq!(failed.history[0].play_count, 1);
        assert_eq!(failed.gigs.rules[&(b, 1)].fulfilled_at_ms, None);
        store
            .connection
            .execute_batch("DROP TRIGGER refuse_rule;")
            .unwrap();
        store.save_delta(&delta).unwrap();
        library.acknowledge(delta.revision);
        library.record_play(1, 30, 0).unwrap();
        store.save_delta(&library.changes()).unwrap();
        // A whole-catalog save exercises replacement and its SQLite backup too.
        store.save(&library.catalog()).unwrap();
        drop(store);
        let (_store, mut reopened, _) = Store::open(path).unwrap();
        let catalog = reopened.catalog();
        assert_eq!(catalog.history[0].play_count, 3);
        assert_eq!(catalog.gigs.rules[&(b, 1)].fulfilled_at_ms, Some(25));
        assert_eq!(catalog.gigs.rules[&(b, 1)].reason, "Birthday");
        reopened.select_gig(Some(a)).unwrap();
        assert_eq!(reopened.gig_rule(a, 1).unwrap().reason, "Opening hour");
        assert!(reopened.gig_rule(a, 1).unwrap().fulfilled_at_ms.is_none());
    }
}
