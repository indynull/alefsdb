//! Golden operator × type matrix for AlefQL.

#[cfg(test)]
mod tests {
    use crate::execute;
    use alefs_namespace::Database;
    use alefs_storage::MemoryStorage;
    use alefs_types::{DbPath, Scalar, Value};
    use std::collections::BTreeMap;

    fn db() -> Database<MemoryStorage> {
        let mut db = Database::open(MemoryStorage::new()).unwrap();
        db.mkdir(&DbPath::parse("/d").unwrap()).unwrap();

        db.set(&DbPath::parse("/d/s").unwrap(), Value::string("hello"))
            .unwrap();
        db.set(&DbPath::parse("/d/i").unwrap(), Value::int(42))
            .unwrap();
        db.set(&DbPath::parse("/d/b").unwrap(), Value::bool(true))
            .unwrap();

        let mut h = BTreeMap::new();
        h.insert("email".into(), Value::string("a@b.c"));
        h.insert("age".into(), Value::int(3));
        db.set(&DbPath::parse("/d/h").unwrap(), Value::Hash(h))
            .unwrap();

        db.set(
            &DbPath::parse("/d/l").unwrap(),
            Value::List(vec![Value::string("login"), Value::int(1)]),
        )
        .unwrap();

        db.set(
            &DbPath::parse("/d/set").unwrap(),
            Value::Set(vec![Value::string("admin"), Value::string("user")]),
        )
        .unwrap();

        let mut t = BTreeMap::new();
        t.insert(Scalar::Int(10), Value::string("ten"));
        t.insert(Scalar::Int(20), Value::string("twenty"));
        db.set(&DbPath::parse("/d/t").unwrap(), Value::Tree(t))
            .unwrap();

        db.set(&DbPath::parse("/d/tmp.bak").unwrap(), Value::string("x"))
            .unwrap();
        db
    }

    fn paths(q: &str) -> Vec<String> {
        let db = db();
        execute(&db, q)
            .unwrap()
            .into_iter()
            .map(|h| h.path.as_str())
            .collect()
    }

    #[test]
    fn path_glob() {
        let p = paths("path /d/*");
        assert!(p.contains(&"/d/s".into()));
        assert!(!p.iter().any(|x| x == "/"));
    }

    #[test]
    fn type_filters() {
        assert_eq!(paths("type int"), vec!["/d/i".to_string()]);
        assert!(paths("type hash").contains(&"/d/h".into()));
        assert!(paths("type scalar").contains(&"/d/s".into()));
    }

    #[test]
    fn value_cmp() {
        assert_eq!(paths("type int AND value = 42"), vec!["/d/i".to_string()]);
        assert!(paths("value = 999").is_empty());
    }

    #[test]
    fn has_and_contains() {
        assert_eq!(
            paths(r#"type hash AND has "email""#),
            vec!["/d/h".to_string()]
        );
        assert!(paths(r#"type set AND contains "admin""#).contains(&"/d/set".into()));
        assert!(paths(r#"type list AND contains "login""#).contains(&"/d/l".into()));
    }

    #[test]
    fn at_and_key_and_size() {
        assert!(paths(r#"type list AND at 0 = "login""#).contains(&"/d/l".into()));
        assert!(paths("type tree AND key >= 15").contains(&"/d/t".into()));
        assert!(paths("type set AND size = 2").contains(&"/d/set".into()));
    }

    #[test]
    fn name_glob_and_not() {
        assert!(paths(r#"name "*.bak""#).contains(&"/d/tmp.bak".into()));
        let p = paths("NOT type scalar");
        assert!(p.contains(&"/d/h".into()));
        assert!(!p.contains(&"/d/s".into()));
    }

    #[test]
    fn wrong_type_is_no_match() {
        // has on scalar → no match, not error
        assert!(paths(r#"path /d/s AND has "x""#).is_empty());
        assert!(paths(r#"path /d/s AND at 0 = 1"#).is_empty());
        assert!(paths(r#"path /d/s AND key = 1"#).is_empty());
    }

    #[test]
    fn boolean_composition() {
        let p = paths(r#"type int OR type bool"#);
        assert!(p.contains(&"/d/i".into()));
        assert!(p.contains(&"/d/b".into()));
        let p = paths(r#"type hash AND has "email" AND path /d/**"#);
        assert_eq!(p, vec!["/d/h".to_string()]);
    }
}
