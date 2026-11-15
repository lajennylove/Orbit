# Changelog

## 0.4.0 - 2026-11-15

### Added

- Blueprint: `medium_text`, `long_text`.

## 0.3.0 - 2026-11-14

### Added

- `schema::table` (ALTER TABLE) with add, change, rename and drop column, index, unique, primary key and foreign key operations.
- `schema::create_if_not_exists`.
- `schema::statement` and `Statement::only(Dialect)` for a raw statement that runs on one database. Orbit's own generated code never uses it.
- Blueprint: `binary`, `timestamp`, `primary([..])`, `rename_column`, `drop_column`, `drop_index(name)`, `drop_foreign(name)`.
- Column: `change`, `primary`, `precision`, `use_current`.
- `Index::name` and `ForeignKey::name`, and a public `Dialect`.

### Changed

- `index` and `unique` on a Blueprint now return `&mut Index`, so a call that ends in `t.unique([..])` with no `;` no longer compiles. Add a `;` to such calls.

## 0.2.0 - 2026-11-14

### Added

- Database-neutral schema builder: `orbit_orm::schema::create`, `schema::drop` and `schema::drop_if_exists`, with a `Blueprint` that describes columns, indexes and foreign keys once. `Statement::to_sql` and `Statement::try_to_sql` render it for SQLite, MySQL and MariaDB, and PostgreSQL.
- `Migration::schema`: a migration can build its `up` and `down` with the schema builder, and the statements are rendered for the connected database.

### Changed

- Migrations carry schema builder statements rendered for the database they run on.
