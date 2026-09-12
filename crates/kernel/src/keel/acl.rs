use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement, Value};

pub fn child_path(parent_path: &str, id: &str) -> String {
    format!("{parent_path}{id}/")
}

pub fn is_within(path: &str, ancestor_path: &str) -> bool {
    path.starts_with(ancestor_path)
}

pub async fn bump_subtree<C: ConnectionTrait>(db: &C, path: &str) -> Result<u64, DbErr> {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE keel_principal SET acl_gen = acl_gen + 1 \
             WHERE account_id IN (SELECT id FROM keel_account WHERE path LIKE $1 || '%')",
            [Value::from(path)],
        ))
        .await?;
    Ok(result.rows_affected())
}

pub async fn move_descendants<C: ConnectionTrait>(
    db: &C,
    id: &str,
    old_path: &str,
    new_path: &str,
) -> Result<u64, DbErr> {
    let cut = i32::try_from(old_path.len()).map_err(|_| DbErr::Custom("path too long".into()))? + 1;
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE keel_account SET path = $1 || substr(path, $2) \
             WHERE path LIKE $3 || '%' AND id <> $4",
            [
                Value::from(new_path),
                Value::from(cut),
                Value::from(old_path),
                Value::from(id),
            ],
        ))
        .await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_child_path_extends_the_parent_path_by_the_id_and_a_slash() {
        assert_eq!(
            child_path("/", "01J8A0000000000000000000A1"),
            "/01J8A0000000000000000000A1/"
        );
        assert_eq!(
            child_path("/01J8A0000000000000000000A1/", "01J8A0000000000000000000A2"),
            "/01J8A0000000000000000000A1/01J8A0000000000000000000A2/"
        );
    }

    #[test]
    fn a_path_is_within_its_own_subtree_and_within_every_ancestor() {
        let reseller = "/01J8A0000000000000000000A1/";
        let customer = "/01J8A0000000000000000000A1/01J8A0000000000000000000A2/";
        assert!(is_within(customer, reseller));
        assert!(is_within(reseller, reseller));
        assert!(is_within(customer, "/"));
        assert!(!is_within(reseller, customer));
        assert!(!is_within("/01J8A0000000000000000000A3/", reseller));
    }
}
