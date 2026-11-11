# Tern

**Tern** is a declarative SQL schema diffing and migration CLI written in Rust.

Instead of manually writing brittle `ALTER TABLE` scripts, developers maintain a single canonical view of their database schema in `.sql` files. Tern spins up an ephemeral shadow database container via **Docker** or **Podman** (or a local temporary database for **SQLite**), replays existing migrations, introspects the live schema, and automatically generates timestamped SQL migration scripts with cryptographic SHA-256 lockfile verification.

---

## Key Features

- **Declarative Schema Management:** Edit your target `.sql` schema files directly; Tern calculates the `CREATE`, `ALTER`, and `DROP` statements required to transition your database.
- **Multi-Engine Shadow Databases:** Automatically detects and orchestrates **Docker** or **Podman** sockets to spin up isolated shadow instances before tearing them down cleanly.
- **Multi-Dialect Support:** First-class support for **PostgreSQL**, **MySQL**, and **SQLite**.
- **Comprehensive Introspection:** Diffs tables, columns, data types, `NOT NULL` modifiers, primary keys, foreign key constraints, and secondary/unique indexes.
- **Cryptographic Integrity (`tern.lock`):** Tracks SHA-256 hashes for every migration file plus a cumulative chain hash to immediately detect tampering, out-of-order modifications, or missing files.
- **Transactional Execution:** Applies pending migrations atomically and records execution history inside an internal `_tern_migrations` state table.

---

## Prerequisites

- **Rust Toolchain:** Rust `1.75+` and Cargo installed (`rustup`).
- **Container Runtime:** **Docker** or **Podman** running locally (required for PostgreSQL and MySQL shadow databases; not required for SQLite).

---

## Installation

Clone the repository and install the `tern` binary globally into your Cargo path (`~/.cargo/bin`):

```bash
git clone [https://github.com/your-username/tern.git](https://github.com/your-username/tern.git)
cd tern
cargo install --path .
```

Verify the installation:

```bash
tern --help
```

---

## Quick Start & Workflow

### 1. Initialize a Project (`tern init`)

Run the interactive configuration wizard at the root of your project:

```bash
tern init
```

This prompts you for your database dialect (`postgres`, `mysql`, or `sqlite`), preferred container runtime (`docker` or `podman`), target schema path, migrations folder, and table filters, generating a `tern.toml` file:

```toml
[project]
db_type = "postgres"
engine = "docker"
target_url = "postgres://postgres:password@localhost:5432/mydb"

[paths]
schema_path = "schema"
migrations_dir = "migrations"

[filter]
include_tables = []
exclude_tables = ["_tern_migrations", "_sqlx_migrations"]
```

### 2. Define Your Declarative Schema

Create or edit `.sql` files inside your configured `schema` directory (e.g., `schema/01_users.sql`):

```sql
CREATE TABLE users (
    id UUID PRIMARY KEY,
    username VARCHAR(50) NOT NULL UNIQUE,
    email VARCHAR(255) NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_users_email ON users (email);
```

### 3. Generate a Migration (`tern generate`)

When your declarative schema changes, generate a new migration script:

```bash
tern generate --name add_user_email
```

**What happens under the hood:**

1. Verifies existing migration checksums against `migrations/tern.lock`.
2. Pulls and starts an ephemeral database container via Docker or Podman.
3. Replays all existing `.sql` files in `migrations/` chronologically.
4. Introspects the shadow database and diffs it against your `schema/` directory.
5. Writes the resulting SQL diff to `migrations/<YYYYMMDDHHMMSS>_add_user_email.sql` and updates `migrations/tern.lock`.
6. Stops and removes the temporary shadow container.

### 4. Check Migration Status (`tern status`)

Inspect which migrations have been applied to your target development or production database:

```bash
tern status
# Or override the target database URL via CLI flag or DATABASE_URL env var:
tern status --url postgres://postgres:password@localhost:5432/prod_db
```

Example output:

```text
MIGRATION FILE                         STATUS         APPLIED AT
--------------------------------------------------------------------------------
20261110120000_initial_schema.sql      Applied        2026-11-10T12:01:05+00:00
20261111075500_add_user_email.sql      Pending        -
--------------------------------------------------------------------------------
Summary: 1 applied, 1 pending.
```

### 5. Apply Migrations (`tern apply`)

Preview pending migrations using `--dry-run`, or execute them transactionally against your target database:

```bash
# Preview pending migrations without executing
tern apply --dry-run

# Apply pending migrations
tern apply
```

---

## CLI Reference

| Command         | Description                                                | Key Flags                                    |
| :-------------- | :--------------------------------------------------------- | :------------------------------------------- |
| `tern init`     | Interactive wizard to generate `tern.toml` and directories | None                                         |
| `tern generate` | Provision shadow DB, replay migrations, and emit SQL diff  | `-n, --name <NAME>`, `-e, --engine <ENGINE>` |
| `tern status`   | Compare local migrations and hashes against target DB      | `-u, --url <DATABASE_URL>`                   |
| `tern apply`    | Execute pending migrations inside transactions             | `-u, --url <DATABASE_URL>`, `--dry-run`      |

---

## Project Architecture

```text
tern/
├── Cargo.toml          # Package manifest and dependencies
├── README.md           # Project documentation
├── tern.toml           # Project configuration (generated via `tern init`)
└── src/
    ├── main.rs         # Entry point and command lifecycle router
    ├── cli.rs          # Clap argument definitions and interactive `inquire` prompts
    ├── config.rs       # `tern.toml` parser and SHA-256 `tern.lock` integrity engine
    ├── engine.rs       # Bollard Docker/Podman container provisioning and teardown
    ├── db.rs           # SQLx connection pooling, shadow replay, and `_tern_migrations` state
    └── differential_engine/ # Modular schema diffing engine & database adapters
        ├── mod.rs      # Engine orchestrator and diff generator
        ├── model.rs    # Normalized database schema models
        ├── parser.rs   # Declarative SQL schema parser and entity extractors
        ├── topological.rs # Dependency resolution & topological sorting
        └── adapters/   # Database dialect adapters (Postgres, MySQL, SQLite)
```

---

## License

MIT
