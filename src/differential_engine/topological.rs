use super::model::NormalizedTable;
use std::collections::{BTreeMap, HashSet};

/// Performs a topological sort of tables based on foreign key dependencies.
/// Independent tables (or tables that are referenced by others) appear earlier in the list,
/// while dependent tables appear later.
/// Handles and breaks circular dependencies safely.
pub fn topological_sort(tables: &BTreeMap<String, NormalizedTable>) -> Vec<String> {
    let mut sorted = Vec::new();
    let mut visited = HashSet::new();
    let mut visiting = HashSet::new();

    fn visit(
        table: &str,
        tables: &BTreeMap<String, NormalizedTable>,
        visited: &mut HashSet<String>,
        visiting: &mut HashSet<String>,
        sorted: &mut Vec<String>,
    ) {
        if visited.contains(table) {
            return;
        }
        // Break circular references to avoid infinite loops
        if visiting.contains(table) {
            return;
        }

        visiting.insert(table.to_string());

        if let Some(t) = tables.get(table) {
            for fk in &t.foreign_keys {
                // Ignore self-references during table creation ordering
                if fk.foreign_table != table {
                    visit(&fk.foreign_table, tables, visited, visiting, sorted);
                }
            }
        }

        visiting.remove(table);
        visited.insert(table.to_string());
        sorted.push(table.to_string());
    }

    for table in tables.keys() {
        visit(table, tables, &mut visited, &mut visiting, &mut sorted);
    }

    sorted
}
