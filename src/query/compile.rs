use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Debug, Formatter};
use std::ops::Sub;

use anyhow::Result;
use itertools::Itertools;

use crate::{EntityId, Validity};
use crate::data::attr::Attribute;
use crate::data::json::JsonValue;
use crate::data::keyword::Keyword;
use crate::data::value::DataValue;
use crate::query::relation::Relation;
use crate::runtime::temp_store::TempStore;
use crate::runtime::transact::SessionTx;

/// example ruleset in python and javascript
/// ```python
/// [
///     R.ancestor(["?a", "?b"],
///         T.parent("?a", "?b")),
///     R.ancestor(["?a", "?b"],
///         T.parent("?a", "?c"),
///         R.ancestor("?c", "?b")),
///     Q(["?a"],
///         R.ancestor("?a", {"name": "Anne"}))
/// ]
///
/// [
///     Q.at("1990-01-01")(["?old_than_anne"],
///         T.age({"name": "Anne"}, "?anne_age"),
///         T.age("?older_than_anne", "?age"),
///         Gt("?age", "?anne_age"))
/// ]
/// ```
/// we also have `F.is_married(["anne", "brutus"], ["constantine", "delphi"])` for ad-hoc fact rules
#[derive(Debug, thiserror::Error)]
pub enum QueryCompilationError {
    #[error("error parsing query clause {0}: {1}")]
    UnexpectedForm(JsonValue, String),
    #[error("arity mismatch for rule {0}: all definitions must have the same arity")]
    ArityMismatch(Keyword),
    #[error("encountered undefined rule {0}")]
    UndefinedRule(Keyword),
    #[error("safety: unbound variables {0:?}")]
    UnsafeUnboundVars(BTreeSet<Keyword>),
    #[error("program logic error: {0}")]
    LogicError(String),
    #[error("entry not found: expect a rule named '?'")]
    EntryNotFound,
}

#[derive(Clone, Debug)]
pub(crate) enum Term<T> {
    Var(Keyword),
    Const(T),
}

impl<T> Term<T> {
    pub(crate) fn get_var(&self) -> Option<&Keyword> {
        match self {
            Self::Var(k) => Some(k),
            Self::Const(_) => None,
        }
    }
    pub(crate) fn get_const(&self) -> Option<&T> {
        match self {
            Self::Const(v) => Some(v),
            Self::Var(_) => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AttrTripleAtom {
    pub(crate) attr: Attribute,
    pub(crate) entity: Term<EntityId>,
    pub(crate) value: Term<DataValue>,
}

#[derive(Clone, Debug)]
pub struct RuleApplyAtom {
    pub(crate) name: Keyword,
    pub(crate) args: Vec<Term<DataValue>>,
}

#[derive(Clone, Debug)]
pub struct PredicateAtom {
    pub(crate) left: Term<DataValue>,
    pub(crate) right: Term<DataValue>,
}

#[derive(Clone, Debug)]
pub(crate) enum Expr {
    Const(Term<DataValue>),
}

#[derive(Clone, Debug)]
pub enum Atom {
    AttrTriple(AttrTripleAtom),
    Rule(RuleApplyAtom),
    Predicate(PredicateAtom),
}

#[derive(Clone, Debug)]
pub struct RuleSet {
    pub(crate) rules: Vec<Rule>,
    pub(crate) arity: usize,
}

impl Rule {
    pub(crate) fn contained_rules(&self) -> BTreeSet<Keyword> {
        let mut collected = BTreeSet::new();
        for clause in &self.body {
            if let Atom::Rule(rule) = clause {
                collected.insert(rule.name.clone());
            }
            // todo: negation, disjunction, etc
        }
        collected
    }
}

pub(crate) type DatalogProgram = BTreeMap<Keyword, RuleSet>;

#[derive(Clone, Debug, Default)]
pub enum Aggregation {
    #[default]
    None,
}

#[derive(Clone, Debug)]
pub(crate) struct Rule {
    pub(crate) head: Vec<BindingHeadTerm>,
    pub(crate) body: Vec<Atom>,
    pub(crate) vld: Validity,
}

#[derive(Clone, Debug)]
pub(crate) struct BindingHeadTerm {
    pub(crate) name: Keyword,
    pub(crate) aggr: Aggregation,
}

pub(crate) struct BindingHeadFormatter<'a>(pub(crate) &'a [BindingHeadTerm]);

impl Debug for BindingHeadFormatter<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = self
            .0
            .iter()
            .map(|h| h.name.to_string_no_prefix())
            .join(", ");
        write!(f, "[{}]", s)
    }
}

impl SessionTx {
    pub(crate) fn compile_rule_body(
        &mut self,
        clauses: &[Atom],
        vld: Validity,
        stores: &BTreeMap<Keyword, (TempStore, usize)>,
        ret_vars: &[Keyword],
    ) -> Result<Relation> {
        let mut ret = Relation::unit();
        let mut seen_variables = BTreeSet::new();
        let mut id_serial = 0;
        let mut gen_temp_kw = || -> Keyword {
            let s = format!("*{}", id_serial);
            let kw = Keyword::from(&s as &str);
            id_serial += 1;
            kw
        };
        for clause in clauses {
            match clause {
                Atom::AttrTriple(a_triple) => match (&a_triple.entity, &a_triple.value) {
                    (Term::Const(eid), Term::Var(v_kw)) => {
                        let temp_join_key_left = gen_temp_kw();
                        let temp_join_key_right = gen_temp_kw();
                        let const_rel = Relation::singlet(
                            vec![temp_join_key_left.clone()],
                            vec![DataValue::EnId(*eid)],
                        );
                        if ret.is_unit() {
                            ret = const_rel;
                        } else {
                            ret = ret.cartesian_join(const_rel);
                        }

                        let mut join_left_keys = vec![temp_join_key_left];
                        let mut join_right_keys = vec![temp_join_key_right.clone()];

                        let v_kw = {
                            if seen_variables.contains(v_kw) {
                                let ret = gen_temp_kw();
                                // to_eliminate.insert(ret.clone());
                                join_left_keys.push(v_kw.clone());
                                join_right_keys.push(ret.clone());
                                ret
                            } else {
                                seen_variables.insert(v_kw.clone());
                                v_kw.clone()
                            }
                        };
                        let right =
                            Relation::triple(a_triple.attr.clone(), vld, temp_join_key_right, v_kw);
                        debug_assert_eq!(join_left_keys.len(), join_right_keys.len());
                        ret = ret.join(right, join_left_keys, join_right_keys);
                    }
                    (Term::Var(e_kw), Term::Const(val)) => {
                        let temp_join_key_left = gen_temp_kw();
                        let temp_join_key_right = gen_temp_kw();
                        let const_rel =
                            Relation::singlet(vec![temp_join_key_left.clone()], vec![val.clone()]);
                        if ret.is_unit() {
                            ret = const_rel;
                        } else {
                            ret = ret.cartesian_join(const_rel);
                        }

                        let mut join_left_keys = vec![temp_join_key_left];
                        let mut join_right_keys = vec![temp_join_key_right.clone()];

                        let e_kw = {
                            if seen_variables.contains(&e_kw) {
                                let ret = gen_temp_kw();
                                join_left_keys.push(e_kw.clone());
                                join_right_keys.push(ret.clone());
                                ret
                            } else {
                                seen_variables.insert(e_kw.clone());
                                e_kw.clone()
                            }
                        };
                        let right =
                            Relation::triple(a_triple.attr.clone(), vld, e_kw, temp_join_key_right);
                        debug_assert_eq!(join_left_keys.len(), join_right_keys.len());
                        ret = ret.join(right, join_left_keys, join_right_keys);
                    }
                    (Term::Var(e_kw), Term::Var(v_kw)) => {
                        let mut join_left_keys = vec![];
                        let mut join_right_keys = vec![];
                        if e_kw == v_kw {
                            unimplemented!();
                        }
                        let e_kw = {
                            if seen_variables.contains(&e_kw) {
                                let ret = gen_temp_kw();
                                join_left_keys.push(e_kw.clone());
                                join_right_keys.push(ret.clone());
                                ret
                            } else {
                                seen_variables.insert(e_kw.clone());
                                e_kw.clone()
                            }
                        };
                        let v_kw = {
                            if seen_variables.contains(v_kw) {
                                let ret = gen_temp_kw();
                                join_left_keys.push(v_kw.clone());
                                join_right_keys.push(ret.clone());
                                ret
                            } else {
                                seen_variables.insert(v_kw.clone());
                                v_kw.clone()
                            }
                        };
                        let right = Relation::triple(a_triple.attr.clone(), vld, e_kw, v_kw);
                        if ret.is_unit() {
                            ret = right;
                        } else {
                            debug_assert_eq!(join_left_keys.len(), join_right_keys.len());
                            ret = ret.join(right, join_left_keys, join_right_keys);
                        }
                    }
                    (Term::Const(eid), Term::Const(val)) => {
                        let (left_var_1, left_var_2) = (gen_temp_kw(), gen_temp_kw());
                        let const_rel = Relation::singlet(
                            vec![left_var_1.clone(), left_var_2.clone()],
                            vec![DataValue::EnId(*eid), val.clone()],
                        );
                        if ret.is_unit() {
                            ret = const_rel;
                        } else {
                            ret = ret.cartesian_join(const_rel);
                        }
                        let (right_var_1, right_var_2) = (gen_temp_kw(), gen_temp_kw());

                        let right = Relation::triple(
                            a_triple.attr.clone(),
                            vld,
                            right_var_1.clone(),
                            right_var_2.clone(),
                        );
                        ret = ret.join(
                            right,
                            vec![left_var_1.clone(), left_var_2.clone()],
                            vec![right_var_1.clone(), right_var_2.clone()],
                        );
                    }
                },
                Atom::Rule(rule_app) => {
                    let (store, arity) = stores
                        .get(&rule_app.name)
                        .ok_or_else(|| QueryCompilationError::UndefinedRule(rule_app.name.clone()))?
                        .clone();
                    if arity != rule_app.args.len() {
                        return Err(
                            QueryCompilationError::ArityMismatch(rule_app.name.clone()).into()
                        );
                    }

                    let mut prev_joiner_vars = vec![];
                    let mut temp_left_bindings = vec![];
                    let mut temp_left_joiner_vals = vec![];
                    let mut right_joiner_vars = vec![];
                    let mut right_vars = vec![];

                    for term in &rule_app.args {
                        match term {
                            Term::Var(var) => {
                                if seen_variables.contains(var) {
                                    prev_joiner_vars.push(var.clone());
                                    let rk = gen_temp_kw();
                                    right_vars.push(rk.clone());
                                    right_joiner_vars.push(rk);
                                } else {
                                    seen_variables.insert(var.clone());
                                    right_vars.push(var.clone());
                                }
                            }
                            Term::Const(constant) => {
                                temp_left_joiner_vals.push(constant.clone());
                                let left_kw = gen_temp_kw();
                                prev_joiner_vars.push(left_kw.clone());
                                temp_left_bindings.push(left_kw);
                                let right_kw = gen_temp_kw();
                                right_joiner_vars.push(right_kw.clone());
                                right_vars.push(right_kw);
                            }
                        }
                    }

                    if !temp_left_joiner_vals.is_empty() {
                        let const_joiner =
                            Relation::singlet(temp_left_bindings, temp_left_joiner_vals);
                        ret = ret.cartesian_join(const_joiner);
                    }

                    let right = Relation::derived(right_vars, store);
                    debug_assert_eq!(prev_joiner_vars.len(), right_joiner_vars.len());
                    ret = ret.join(right, prev_joiner_vars, right_joiner_vars);
                }
                Atom::Predicate(_) => {
                    todo!()
                }
            }
        }

        let ret_vars_set = ret_vars.iter().cloned().collect();

        ret.eliminate_temp_vars(&ret_vars_set)?;
        let cur_ret_set: BTreeSet<_> = ret.bindings().into_iter().collect();
        if cur_ret_set != ret_vars_set {
            ret = ret.cartesian_join(Relation::unit());
            ret.eliminate_temp_vars(&ret_vars_set)?;
        }

        let cur_ret_set: BTreeSet<_> = ret.bindings().into_iter().collect();
        if cur_ret_set != ret_vars_set {
            let diff = cur_ret_set.sub(&cur_ret_set);
            return Err(QueryCompilationError::UnsafeUnboundVars(diff).into());
        }
        let cur_ret_bindings = ret.bindings();
        if ret_vars != cur_ret_bindings {
            ret = ret.reorder(ret_vars.to_vec());
        }

        Ok(ret)
    }
}
