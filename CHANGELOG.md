# Changelog

## 0.2.0 - 2026-11-14

### Added

- Database-neutral schema builder: `orbit_orm::schema::create`, `schema::drop` and `schema::drop_if_exists`, with a `Blueprint` that describes columns, indexes and foreign keys once. `Statement::to_sql` and `Statement::try_to_sql` render it for SQLite, MySQL and MariaDB, and PostgreSQL.
- `Migration::schema`: a migration can build its `up` and `down` with the schema builder, and the statements are rendered for the connected database.

### Changed

- Migrations carry schema builder statements rendered for the database they run on.
