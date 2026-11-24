//! A take replaces just its chosen target. Writes compare the prior complete book inside SQLite.
use super::*;
use crate::sampler::phrase::Patterns;
const KEY: &str = "sampler_patterns_v1";
pub(super) fn read(db: &Connection) -> Result<Patterns, String> {
    let raw:Option<String>=db.query_row("SELECT CASE WHEN length(CAST(value AS BLOB))<=32768 THEN value ELSE '' END FROM library_meta WHERE key=?1",[KEY],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
    let book: Patterns = match raw {
        Some(raw) => serde_json::from_str(&raw)
            .map_err(|e| format!("Invalid saved sampler patterns: {e}"))?,
        None => Default::default(),
    };
    book.validate()?;
    Ok(book)
}
pub(super) struct Write {
    expected: Patterns,
    proposed: Patterns,
    reply: mpsc::Sender<Result<(), String>>,
}
impl Write {
    pub(super) fn run(self, store: &mut Store) {
        let result = (|| {
            self.proposed.validate()?;
            let tx = store
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|e| e.to_string())?;
            let app: i32 = tx
                .pragma_query_value(None, "application_id", |r| r.get(0))
                .map_err(|e| e.to_string())?;
            let version: i32 = tx
                .pragma_query_value(None, "user_version", |r| r.get(0))
                .map_err(|e| e.to_string())?;
            if app != APPLICATION_ID || version != SCHEMA_VERSION {
                return Err("Library changed; sampler take preserved.".into());
            }
            if read(&tx)? != self.expected {
                return Err(
                    "The saved patterns changed. Reopen the library before replacing a target."
                        .to_owned(),
                );
            }
            let raw = serde_json::to_string(&self.proposed).map_err(|e| e.to_string())?;
            if raw.len() > 32768 {
                return Err("Sampler pattern size exceeds the limit.".into());
            }
            tx.execute("INSERT INTO library_meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",rusqlite::params![KEY,raw]).map_err(|e|e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            store.backup_pending = true;
            Ok(())
        })();
        let _ = self.reply.send(result);
    }
}
impl Worker {
    pub(crate) fn save_patterns(
        &self,
        expected: Patterns,
        proposed: Patterns,
    ) -> Result<Receiver<Result<(), String>>, String> {
        let (reply, rx) = mpsc::channel();
        self.tx
            .try_send(Command::Pattern(Box::new(Write {
                expected,
                proposed,
                reply,
            })))
            .map_err(|e| e.to_string())?;
        Ok(rx)
    }
}

#[cfg(test)]
mod qualification {
    use super::*;
    use crate::sampler::{
        phrase::Take,
        tools::{LayerMix, LayerPair, Tuning},
        Preferences,
    };

    #[test]
    fn m09_saved_take_and_m02_a04_sample_setup_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("library.sqlite3");
        let (mut store, _, _) = Store::open(db.clone()).unwrap();
        let before = read(&store.connection).unwrap();
        let start = std::time::Instant::now();
        let mut take = Take::new(false, 120, start).unwrap();
        for (i, ms) in [0, 250, 375, 750].into_iter().enumerate() {
            take.hit(i as u8, start + Duration::from_millis(ms));
        }
        assert_eq!(read(&store.connection).unwrap(), before);
        let saved = Patterns {
            base: take.pattern,
            ..before.clone()
        };
        let write = |store: &mut Store| {
            let (reply, rx) = mpsc::channel();
            Write {
                expected: before.clone(),
                proposed: saved.clone(),
                reply,
            }
            .run(store);
            rx.recv().unwrap()
        };
        write(&mut store).unwrap();
        assert!(write(&mut store).is_err());
        let mut setup = Preferences::default();
        setup.paths[1] = Some(dir.path().join("kick.wav"));
        setup.tuning[1] = Some(Tuning {
            root: 69,
            correction: -25,
            digest: [42; 32],
        });
        setup.layers[1] = Some(LayerPair {
            name: "Kick with clap".into(),
            second_path: dir.path().join("clap.wav"),
            mixes: [LayerMix::default(), LayerMix::new(20, 50, -100).unwrap()],
        });
        store.save_sampler(&setup).unwrap();
        drop(store);
        let (store, _, _) = Store::open(db).unwrap();
        assert_eq!(read(&store.connection).unwrap(), saved);
        assert_eq!(store.read_sampler().unwrap(), setup);
    }
}
