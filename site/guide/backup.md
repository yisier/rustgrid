# Backup & restore

RustGrid's own backup format is `.rgbak`: one file holding the schema and the rows, written as compressed chunks with a JSON manifest. It is independent of the server's dump tools and restores on another machine.

![New Backup: target, object selection and profile name](./assets/en/backup.png)

## Create a backup

1. Select a database and open the **Backups** main tab.
2. Click **New Backup**. The window targets that connection and database.
3. On **Object Selection**, tick tables, views and routines. **Select All** / **Deselect All** speed up a large schema.
4. Optionally name the configuration so the same selection can be rerun, then click **Start**.

The **Info Log** tab shows object and record counts, live messages, elapsed time and the final success / failure totals. The progress bar turns green when the run completes.

## Restore and extract

- **Restore Backup** writes the selected backup back to the chosen database. Existing objects are recreated; rows are inserted through the engine's normal write path.
- **Extract SQL** turns a backup into a readable script, useful for review or for moving the data through another tool.
- A saved configuration keeps the target and object selection. Reopen it from the list to run the same job again.

## The backup list

- Each backup shows its name, modification date, file size and note. Click a header to sort.
- **F2** renames a backup, the toolbar deletes it after confirmation, and **Refresh** reloads the list.
- Backups are stored per database in the configuration directory and follow the connection profile, not the server.

## Scope

Backup and restore cover MySQL/MariaDB, PostgreSQL, SQL Server, Oracle and SQLite. ClickHouse, Redis and MongoDB do not offer `.rgbak` backup in this version.

Take a backup before [Sync](./sync.html), a large import or a destructive table change. Because the file contains both structure and data, it can seed a fresh database or move a schema between environments.

::: warning Restoring into a live database
Restore recreates objects and writes rows. Read the object selection before starting, and use a new database when you only want to inspect the contents. The backup file itself is plain compressed data, so treat it as sensitive as the source database.
:::
