//! Translation of the app's filter tree into a MongoDB query document.
//!
//! The grid's filter builder is engine-agnostic; this module is the only place that knows how its
//! operators map onto MongoDB's query language. Nodes that are disabled or incomplete are skipped,
//! exactly like the SQL drivers skip them, and groups become `$and`/`$or` of their children.

use mongodb::bson::{Bson, Document, doc};
use rustgrid_core::{ColumnInfo, FilterCondition, FilterConjunction, FilterNode, FilterOperator};

use crate::value;

/// Build the query document for one page's filter tree.
pub fn build(nodes: &[FilterNode], columns: &[ColumnInfo]) -> Document {
    combine(nodes, columns).unwrap_or_default()
}

fn combine(nodes: &[FilterNode], columns: &[ColumnInfo]) -> Option<Document> {
    let mut result: Option<Document> = None;
    for node in nodes.iter().filter(|node| node.enabled()) {
        let conjunction = node.conjunction();
        let next = match node {
            FilterNode::Condition(condition) => condition_document(condition, columns),
            FilterNode::Group(group) => combine(&group.children, columns),
        };
        let Some(next) = next else { continue };
        result = Some(match result {
            None => next,
            Some(previous) => match conjunction {
                FilterConjunction::And => doc! { "$and": [previous, next] },
                FilterConjunction::Or => doc! { "$or": [previous, next] },
            },
        });
    }
    result
}

fn condition_document(condition: &FilterCondition, columns: &[ColumnInfo]) -> Option<Document> {
    let field = condition.column.trim();
    if field.is_empty() {
        return None;
    }
    let typed = |text: &str| typed_value(columns, field, text);
    let value = condition.value.trim();
    let value2 = condition.value2.trim();

    let document = match condition.operator {
        FilterOperator::Equal => {
            if value.is_empty() {
                return None;
            }
            doc! { field: typed(value) }
        }
        FilterOperator::NotEqual => {
            if value.is_empty() {
                return None;
            }
            doc! { field: { "$ne": typed(value) } }
        }
        FilterOperator::LessThan => doc! { field: { "$lt": typed(value) } },
        FilterOperator::LessOrEqual => doc! { field: { "$lte": typed(value) } },
        FilterOperator::GreaterThan => doc! { field: { "$gt": typed(value) } },
        FilterOperator::GreaterOrEqual => doc! { field: { "$gte": typed(value) } },
        FilterOperator::Contains => regex_document(field, &escape_regex(value)),
        FilterOperator::NotContains => {
            doc! { field: { "$not": { "$regex": escape_regex(value), "$options": "i" } } }
        }
        FilterOperator::StartsWith => regex_document(field, &format!("^{}", escape_regex(value))),
        FilterOperator::NotStartsWith => {
            doc! { field: { "$not": { "$regex": format!("^{}", escape_regex(value)), "$options": "i" } } }
        }
        FilterOperator::EndsWith => regex_document(field, &format!("{}$", escape_regex(value))),
        FilterOperator::NotEndsWith => {
            doc! { field: { "$not": { "$regex": format!("{}$", escape_regex(value)), "$options": "i" } } }
        }
        FilterOperator::IsNull => doc! { field: Bson::Null },
        FilterOperator::IsNotNull => doc! { field: { "$ne": Bson::Null } },
        FilterOperator::IsEmpty => doc! { field: "" },
        FilterOperator::IsNotEmpty => doc! { field: { "$ne": "" } },
        FilterOperator::Between | FilterOperator::NotBetween => {
            if value.is_empty() || value2.is_empty() {
                return None;
            }
            let range = doc! { "$gte": typed(value), "$lte": typed(value2) };
            if condition.operator == FilterOperator::Between {
                doc! { field: range }
            } else {
                doc! { field: { "$not": range } }
            }
        }
        FilterOperator::InList | FilterOperator::NotInList => {
            let items = condition.list_values();
            if items.is_empty() {
                return None;
            }
            let values: Vec<Bson> = items.iter().map(|item| typed(item)).collect();
            if condition.operator == FilterOperator::InList {
                doc! { field: { "$in": values } }
            } else {
                doc! { field: { "$nin": values } }
            }
        }
    };
    Some(document)
}

fn regex_document(field: &str, pattern: &str) -> Document {
    doc! { field: { "$regex": pattern, "$options": "i" } }
}

/// The typed BSON value for a filter operand. A value that does not parse as the column's type
/// falls back to text (so a numeric-looking string still matches a string field), and `_id`
/// accepts both hex ObjectIds and plain strings.
fn typed_value(columns: &[ColumnInfo], field: &str, text: &str) -> Bson {
    let column_type = columns
        .iter()
        .find(|column| column.name == field)
        .map(|column| column.data_type.as_str())
        .unwrap_or("");
    value::from_text(column_type, text).unwrap_or_else(|_| Bson::String(text.to_string()))
}

/// Escape the characters MongoDB's regex engine treats specially, so `%`-style user input is
/// matched literally.
fn escape_regex(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(
            ch,
            '\\' | '.' | '^' | '$' | '|' | '?' | '*' | '+' | '(' | ')' | '[' | ']' | '{' | '}'
        ) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgrid_core::FilterConjunction;

    fn columns() -> Vec<ColumnInfo> {
        vec![
            ColumnInfo {
                name: "_id".into(),
                data_type: "objectId".into(),
                nullable: false,
                primary_key: true,
                auto_increment: false,
                generated: false,
                comment: String::new(),
            },
            ColumnInfo {
                name: "age".into(),
                data_type: "int".into(),
                nullable: true,
                primary_key: false,
                auto_increment: false,
                generated: false,
                comment: String::new(),
            },
        ]
    }

    fn condition(column: &str, operator: FilterOperator, value: &str) -> FilterNode {
        FilterNode::Condition(FilterCondition {
            column: column.into(),
            operator,
            value: value.into(),
            value2: String::new(),
            conjunction: FilterConjunction::And,
            enabled: true,
        })
    }

    #[test]
    fn equality_uses_the_column_type() {
        let filter = build(
            &[condition("age", FilterOperator::GreaterThan, "20")],
            &columns(),
        );
        assert_eq!(filter, doc! { "age": { "$gt": 20i32 } });
    }

    #[test]
    fn string_contains_is_a_case_insensitive_regex() {
        let filter = build(
            &[condition("name", FilterOperator::Contains, "ab.c")],
            &columns(),
        );
        assert_eq!(
            filter,
            doc! { "name": { "$regex": "ab\\.c", "$options": "i" } }
        );
    }

    #[test]
    fn two_conditions_are_anded() {
        let nodes = vec![
            condition("age", FilterOperator::Equal, "30"),
            condition("name", FilterOperator::Equal, "x"),
        ];
        let filter = build(&nodes, &columns());
        assert_eq!(
            filter,
            doc! { "$and": [ { "age": 30i32 }, { "name": "x" } ] }
        );
    }

    #[test]
    fn disabled_nodes_are_skipped() {
        let mut node = condition("age", FilterOperator::Equal, "30");
        node.set_enabled(false);
        assert_eq!(build(&[node], &columns()), Document::new());
    }

    #[test]
    fn in_list_splits_on_commas() {
        let filter = build(
            &[condition("age", FilterOperator::InList, "30, 40")],
            &columns(),
        );
        assert_eq!(filter, doc! { "age": { "$in": [30i32, 40i32] } });
    }
}
