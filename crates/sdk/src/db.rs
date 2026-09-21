use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr, ExecResult,
    QueryResult, Statement,
};

#[derive(Clone, Debug)]
pub struct Db(Inner);

#[derive(Clone, Debug)]
enum Inner {
    Transaction(Arc<DatabaseTransaction>),
    Connection(DatabaseConnection),
}

impl Db {
    #[doc(hidden)]
    pub fn from_transaction(transaction: DatabaseTransaction) -> Self {
        Self(Inner::Transaction(Arc::new(transaction)))
    }

    #[doc(hidden)]
    pub fn from_connection(connection: DatabaseConnection) -> Self {
        Self(Inner::Connection(connection))
    }

    pub fn is_transaction(&self) -> bool {
        matches!(self.0, Inner::Transaction(_))
    }

    pub fn connection(&self) -> Option<&DatabaseConnection> {
        match &self.0 {
            Inner::Transaction(_) => None,
            Inner::Connection(connection) => Some(connection),
        }
    }

    #[doc(hidden)]
    pub fn into_transaction(self) -> Result<Option<DatabaseTransaction>, usize> {
        match self.0 {
            Inner::Transaction(transaction) => Arc::try_unwrap(transaction)
                .map(Some)
                .map_err(|shared| Arc::strong_count(&shared)),
            Inner::Connection(_) => Ok(None),
        }
    }
}

#[async_trait]
impl ConnectionTrait for Db {
    fn get_database_backend(&self) -> DbBackend {
        match &self.0 {
            Inner::Transaction(transaction) => transaction.get_database_backend(),
            Inner::Connection(connection) => connection.get_database_backend(),
        }
    }

    async fn execute(&self, stmt: Statement) -> Result<ExecResult, DbErr> {
        match &self.0 {
            Inner::Transaction(transaction) => transaction.execute(stmt).await,
            Inner::Connection(connection) => connection.execute(stmt).await,
        }
    }

    async fn execute_unprepared(&self, sql: &str) -> Result<ExecResult, DbErr> {
        match &self.0 {
            Inner::Transaction(transaction) => transaction.execute_unprepared(sql).await,
            Inner::Connection(connection) => connection.execute_unprepared(sql).await,
        }
    }

    async fn query_one(&self, stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
        match &self.0 {
            Inner::Transaction(transaction) => transaction.query_one(stmt).await,
            Inner::Connection(connection) => connection.query_one(stmt).await,
        }
    }

    async fn query_all(&self, stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
        match &self.0 {
            Inner::Transaction(transaction) => transaction.query_all(stmt).await,
            Inner::Connection(connection) => connection.query_all(stmt).await,
        }
    }

    fn support_returning(&self) -> bool {
        match &self.0 {
            Inner::Transaction(transaction) => transaction.support_returning(),
            Inner::Connection(connection) => connection.support_returning(),
        }
    }

    fn is_mock_connection(&self) -> bool {
        match &self.0 {
            Inner::Transaction(transaction) => transaction.is_mock_connection(),
            Inner::Connection(connection) => connection.is_mock_connection(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pool_db_exposes_its_connection_and_a_transaction_db_does_not() {
        let pool = Db::from_connection(DatabaseConnection::Disconnected);
        assert!(!pool.is_transaction());
        assert!(matches!(
            pool.connection(),
            Some(DatabaseConnection::Disconnected)
        ));
        assert!(pool.into_transaction().unwrap().is_none());
    }
}
