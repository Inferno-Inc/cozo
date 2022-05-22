use crate::context::TempDbContext;
use crate::data::eval::{EvalError, PartialEvalContext};
use crate::data::expr::Expr;
use crate::data::parser::ExprParseError;
use crate::data::tuple::{DataKind, OwnTuple};
use crate::data::tuple_set::{BindingMap, TableId, TupleSet, TupleSetIdx};
use crate::data::value::{StaticValue, Value};
use crate::ddl::reify::{AssocInfo, EdgeInfo, IndexInfo, TableInfo};
use crate::parser::{Pair, Pairs, Rule};
use crate::runtime::session::Definable;
use std::collections::BTreeMap;
use std::result;
use std::sync::Arc;

#[derive(thiserror::Error, Debug)]
pub(crate) enum AlgebraParseError {
    #[error("{0} cannot be chained")]
    Unchainable(String),

    #[error("wrong argument count for {0}")]
    WrongArgumentCount(String),

    #[error("wrong argument type for {0}({1}): {2}")]
    WrongArgumentType(String, usize, String),

    #[error(transparent)]
    ExprParse(#[from] ExprParseError),

    #[error(transparent)]
    EvalError(#[from] EvalError),

    #[error("Table not found {0}")]
    TableNotFound(String),

    #[error("Table id not found {0:?}")]
    TableIdNotFound(TableId),

    #[error("Not enough arguments for {0}")]
    NotEnoughArguments(String),

    #[error("Value error {0:?}")]
    ValueError(StaticValue),
}

type Result<T> = result::Result<T, AlgebraParseError>;

pub(crate) trait InterpretContext: PartialEvalContext {
    fn resolve_definable(&self, name: &str) -> Option<Definable>;
    fn resolve_table(&self, name: &str) -> Option<TableId>;
    fn get_table_info(&self, table_id: TableId) -> Result<TableInfo>;
    fn get_table_assocs(&self, table_id: TableId) -> Result<Vec<AssocInfo>>;
    fn get_node_edges(&self, table_id: TableId) -> Result<(Vec<EdgeInfo>, Vec<EdgeInfo>)>;
    fn get_table_indices(&self, table_id: TableId) -> Result<Vec<IndexInfo>>;
}

impl InterpretContext for () {
    fn resolve_definable(&self, _name: &str) -> Option<Definable> {
        None
    }

    fn resolve_table(&self, _name: &str) -> Option<TableId> {
        None
    }

    fn get_table_info(&self, table_id: TableId) -> Result<TableInfo> {
        Err(AlgebraParseError::TableIdNotFound(table_id))
    }

    fn get_table_assocs(&self, table_id: TableId) -> Result<Vec<AssocInfo>> {
        Err(AlgebraParseError::TableIdNotFound(table_id))
    }

    fn get_node_edges(&self, table_id: TableId) -> Result<(Vec<EdgeInfo>, Vec<EdgeInfo>)> {
        Err(AlgebraParseError::TableIdNotFound(table_id))
    }

    fn get_table_indices(&self, table_id: TableId) -> Result<Vec<IndexInfo>> {
        Err(AlgebraParseError::TableIdNotFound(table_id))
    }
}

pub(crate) trait RelationalAlgebra {
    fn name(&self) -> &str;
}

const NAME_RA_FROM_VALUES: &str = "Values";

#[derive(Clone, Debug)]
struct RaFromValues {
    binding: BindingMap,
    values: Vec<TupleSet>,
}

fn assert_rule(pair: &Pair, rule: Rule, name: &str, u: usize) -> Result<()> {
    if pair.as_rule() == rule {
        Ok(())
    } else {
        Err(AlgebraParseError::WrongArgumentType(
            name.to_string(),
            u,
            format!("{:?}", pair.as_rule()),
        ))
    }
}

impl RaFromValues {
    fn build(
        ctx: &impl InterpretContext,
        prev: Option<Arc<dyn RelationalAlgebra>>,
        mut args: Pairs,
    ) -> Result<Self> {
        if !matches!(prev, None) {
            return Err(AlgebraParseError::Unchainable(
                NAME_RA_FROM_VALUES.to_string(),
            ));
        }
        let not_enough_args =
            || AlgebraParseError::NotEnoughArguments(NAME_RA_FROM_VALUES.to_string());
        let schema = args
            .next()
            .ok_or_else(not_enough_args)?
            .into_inner()
            .next()
            .ok_or_else(not_enough_args)?;
        assert_rule(&schema, Rule::scoped_list, NAME_RA_FROM_VALUES, 0)?;
        let mut schema_pairs = schema.into_inner();
        let binding = schema_pairs.next().ok_or_else(not_enough_args)?.as_str();
        let binding_map = schema_pairs
            .enumerate()
            .map(|(i, v)| {
                (
                    v.as_str().to_string(),
                    TupleSetIdx {
                        is_key: false,
                        t_set: 0,
                        col_idx: i,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let n_fields = binding_map.len();
        let binding_map = BTreeMap::from([(binding.to_string(), binding_map)]);
        let data = args
            .next()
            .ok_or_else(not_enough_args)?
            .into_inner()
            .next()
            .ok_or_else(not_enough_args)?;
        assert_rule(&data, Rule::expr, NAME_RA_FROM_VALUES, 1)?;
        let data = Expr::try_from(data)?.interpret_eval(ctx)?.to_static();
        let data = data.into_vec().map_err(AlgebraParseError::ValueError)?;
        let values = data
            .into_iter()
            .map(|v| match v.into_vec() {
                Ok(v) => {
                    if v.len() == n_fields {
                        let mut tuple = OwnTuple::with_data_prefix(DataKind::Data);
                        tuple.extend(v);
                        let mut tset = TupleSet::default();
                        tset.push_val(tuple.into());
                        Ok(tset)
                    } else {
                        Err(AlgebraParseError::ValueError(Value::List(v)))
                    }
                }
                Err(v) => Err(AlgebraParseError::ValueError(v)),
            })
            .collect::<Result<Vec<_>>>()?;
        dbg!(&binding_map);
        dbg!(&values);
        Ok(Self {
            binding: binding_map,
            values,
        })
    }
}

impl RelationalAlgebra for RaFromValues {
    fn name(&self) -> &str {
        NAME_RA_FROM_VALUES
    }
}

const NAME_INSERT: &str = "Insert";

pub(crate) fn build_ra_expr(
    ctx: &impl InterpretContext,
    pair: Pair,
) -> Result<Arc<dyn RelationalAlgebra>> {
    let mut built: Option<Arc<dyn RelationalAlgebra>> = None;
    for pair in pair.into_inner() {
        let mut pairs = pair.into_inner();
        match pairs.next().unwrap().as_str() {
            NAME_INSERT => todo!(),
            NAME_RA_FROM_VALUES => {
                built = Some(Arc::new(RaFromValues::build(ctx, built, pairs)?));
            }
            _ => unimplemented!(),
        }
    }
    Ok(built.unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{CozoParser, Rule};
    use pest::Parser;

    #[test]
    fn parse_ra() -> Result<()> {
        let s = r#"
         Values(v: [id, vals], [[100, 'confidential'], [101, 'top secret']])
        // .Insert(f:Friend)
        "#;
        build_ra_expr(
            &(),
            CozoParser::parse(Rule::ra_expr_all, s)
                .unwrap()
                .into_iter()
                .next()
                .unwrap(),
        )?;

        // let s = r#"
        //  From(f:Person-[:HasJob]->j:Job,
        //       f.id == 101, j.id > 10)
        // .Select(f: {*id: f.id})
        // "#;
        // build_ra_expr(CozoParser::parse(Rule::ra_expr_all, s).unwrap().into_iter().next().unwrap());
        Ok(())
    }
}
