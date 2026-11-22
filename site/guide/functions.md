# Functions & procedures

Stored routines are listed per database and edited in the same SQL editor used by queries. The page calls them *functions*, but it covers stored procedures and packages too.

![The routine editor: definition, info and SQL preview](./assets/en/routines.png)

## The list

- The list shows the routine name, modification time, kind (function or procedure), determinism and comment. Use the search box or switch between the tile and detailed layouts.
- **Design Function** opens the selected routine; **New Function** starts from a template; **Delete Function** asks for confirmation; **Run Function** executes it.
- The connection tree has a **Functions** category with the same actions, so a routine can be opened without the main tab.
- Oracle adds packages and triggers to the same category.

## The routine editor

- **Definition** is the full `CREATE` statement loaded from the engine. It is editable with syntax highlighting, find and word wrap.
- **Information** shows the routine's kind, return type, definer, data access, determinism, SQL mode, character set and collation.

![Routine metadata](./assets/en/routine-info.png)

- **SQL Preview** is the script **Save** will run.
- **Save** drops the original routine and replays the edited `CREATE`. That is how a function's body, parameters and attributes change. **Ctrl+S** saves.
- **Run Function** wraps a function in `SELECT` and a procedure in `CALL`, then opens the call in a new query tab. A routine without arguments runs immediately; one with arguments leaves the call ready to fill in.

## Notes

- The tab title keeps the routine's database and connection, so several routines can stay open side by side.
- Routines are available on MySQL/MariaDB, PostgreSQL, SQL Server and Oracle. SQLite, ClickHouse, Redis and MongoDB have no stored routines, so the category is hidden for them.
- A routine that references a dropped object still opens; the error appears when it runs or when the server revalidates it.
- Unlike a query tab, saving a routine changes the server immediately. Take a [backup](./backup.html) before editing a routine in production.
