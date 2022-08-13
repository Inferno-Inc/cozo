use std::collections::BTreeMap;
use std::env::temp_dir;
use std::fmt::{Debug, Formatter};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::Result;
use either::{Left, Right};
use itertools::Itertools;
use log::debug;
use serde_json::json;
use uuid::Uuid;

use cozorocks::{DbBuilder, DbIter, RawRocksDb, RocksDb};

use crate::data::compare::{rusty_cmp, DB_KEY_PREFIX_LEN};
use crate::data::encode::{
    decode_ea_key, decode_value_from_key, decode_value_from_val, encode_eav_key, StorageTag,
};
use crate::data::id::{AttrId, EntityId, TxId, Validity};
use crate::data::json::JsonValue;
use crate::data::symb::PROG_ENTRY;
use crate::data::triple::StoreOp;
use crate::data::tuple::{rusty_scratch_cmp, SCRATCH_DB_KEY_PREFIX_LEN};
use crate::data::value::DataValue;
use crate::parse::cozoscript::query::parse_query_to_json;
use crate::parse::cozoscript::schema::parse_schema_to_json;
use crate::parse::cozoscript::tx::parse_tx_to_json;
use crate::parse::schema::AttrTxItem;
use crate::query::pull::CurrentPath;
use crate::runtime::transact::SessionTx;

pub struct Db {
    db: RocksDb,
    temp_db: RawRocksDb,
    last_attr_id: Arc<AtomicU64>,
    last_ent_id: Arc<AtomicU64>,
    last_tx_id: Arc<AtomicU64>,
    temp_store_id: Arc<AtomicU32>,
    n_sessions: Arc<AtomicUsize>,
    session_id: usize,
}

impl Debug for Db {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Db<session {}, attrs {:?}, entities {:?}, txs {:?}, sessions: {:?}>",
            self.session_id, self.last_tx_id, self.last_ent_id, self.last_tx_id, self.n_sessions
        )
    }
}

impl Db {
    pub fn build(builder: DbBuilder<'_>) -> Result<Self> {
        let db = builder
            .use_bloom_filter(true, 10., true)
            .use_capped_prefix_extractor(true, DB_KEY_PREFIX_LEN)
            .use_custom_comparator("cozo_rusty_cmp", rusty_cmp, false)
            .build()?;
        let mut temp_db_location = temp_dir();
        temp_db_location.push(format!("{}.cozo", Uuid::new_v4()));

        let scratch = DbBuilder::default()
            .path(temp_db_location.to_str().unwrap())
            .create_if_missing(true)
            .destroy_on_exit(true)
            .use_bloom_filter(true, 10., true)
            .use_capped_prefix_extractor(true, SCRATCH_DB_KEY_PREFIX_LEN)
            .use_custom_comparator("cozo_rusty_scratch_cmp", rusty_scratch_cmp, false)
            .build_raw(true)?
            .ignore_range_deletions(true);
        let ret = Self {
            db,
            temp_db: scratch,
            last_attr_id: Arc::new(Default::default()),
            last_ent_id: Arc::new(Default::default()),
            last_tx_id: Arc::new(Default::default()),
            temp_store_id: Arc::new(Default::default()),
            n_sessions: Arc::new(Default::default()),
            session_id: Default::default(),
        };
        ret.load_last_ids()?;
        Ok(ret)
    }

    pub fn new_session(&self) -> Result<Self> {
        let old_count = self.n_sessions.fetch_add(1, Ordering::AcqRel);

        Ok(Self {
            db: self.db.clone(),
            temp_db: self.temp_db.clone(),
            last_attr_id: self.last_attr_id.clone(),
            last_ent_id: self.last_ent_id.clone(),
            last_tx_id: self.last_tx_id.clone(),
            temp_store_id: self.temp_store_id.clone(),
            n_sessions: self.n_sessions.clone(),
            session_id: old_count + 1,
        })
    }

    fn load_last_ids(&self) -> Result<()> {
        let mut tx = self.transact()?;
        self.last_tx_id
            .store(tx.load_last_tx_id()?.0, Ordering::Release);
        self.last_attr_id
            .store(tx.load_last_attr_id()?.0, Ordering::Release);
        self.last_ent_id
            .store(tx.load_last_entity_id()?.0, Ordering::Release);
        Ok(())
    }
    pub fn transact(&self) -> Result<SessionTx> {
        let ret = SessionTx {
            tx: self.db.transact().set_snapshot(true).start(),
            temp_store: self.temp_db.clone(),
            temp_store_id: self.temp_store_id.clone(),
            w_tx_id: None,
            last_attr_id: self.last_attr_id.clone(),
            last_ent_id: self.last_ent_id.clone(),
            last_tx_id: self.last_tx_id.clone(),
            attr_by_id_cache: Default::default(),
            attr_by_kw_cache: Default::default(),
            temp_entity_to_perm: Default::default(),
            eid_by_attr_val_cache: Default::default(),
            touched_eids: Default::default(),
        };
        Ok(ret)
    }
    pub fn transact_write(&self) -> Result<SessionTx> {
        let last_tx_id = self.last_tx_id.fetch_add(1, Ordering::AcqRel);
        let cur_tx_id = TxId(last_tx_id + 1);

        let ret = SessionTx {
            tx: self.db.transact().set_snapshot(true).start(),
            temp_store: self.temp_db.clone(),
            temp_store_id: self.temp_store_id.clone(),
            w_tx_id: Some(cur_tx_id),
            last_attr_id: self.last_attr_id.clone(),
            last_ent_id: self.last_ent_id.clone(),
            last_tx_id: self.last_tx_id.clone(),
            attr_by_id_cache: Default::default(),
            attr_by_kw_cache: Default::default(),
            temp_entity_to_perm: Default::default(),
            eid_by_attr_val_cache: Default::default(),
            touched_eids: Default::default(),
        };
        Ok(ret)
    }
    pub fn total_iter(&self) -> DbIter {
        let mut it = self.db.transact().start().iterator().start();
        it.seek_to_start();
        it
    }
    pub fn pull(&self, eid: &JsonValue, payload: &JsonValue, vld: &JsonValue) -> Result<JsonValue> {
        let eid = EntityId::try_from(eid)?;
        let vld = match vld {
            JsonValue::Null => Validity::current(),
            v => Validity::try_from(v)?,
        };
        let mut tx = self.transact()?;
        let specs = tx.parse_pull(payload, 0)?;
        let mut collected = Default::default();
        let mut recursive_seen = Default::default();
        for (idx, spec) in specs.iter().enumerate() {
            tx.pull(
                eid,
                vld,
                spec,
                0,
                &specs,
                CurrentPath::new(idx)?,
                &mut collected,
                &mut recursive_seen,
            )?;
        }
        Ok(JsonValue::Object(collected))
    }
    pub fn run_tx_triples(&self, payload: &str) -> Result<JsonValue> {
        let payload = parse_tx_to_json(payload)?;
        self.transact_triples(&payload)
    }
    pub fn transact_triples(&self, payload: &JsonValue) -> Result<JsonValue> {
        let mut tx = self.transact_write()?;
        let (payloads, comment) = tx.parse_tx_requests(payload)?;
        let res: JsonValue = tx
            .tx_triples(payloads)?
            .iter()
            .map(|(eid, size)| json!([eid.0, size]))
            .collect();
        let tx_id = tx.get_write_tx_id()?;
        tx.commit_tx(&comment, false)?;
        Ok(json!({
            "tx_id": tx_id,
            "results": res
        }))
    }
    pub fn run_tx_attributes(&self, payload: &str) -> Result<JsonValue> {
        let payload = parse_schema_to_json(payload)?;
        self.transact_attributes(&payload)
    }
    pub fn transact_attributes(&self, payload: &JsonValue) -> Result<JsonValue> {
        let (attrs, comment) = AttrTxItem::parse_request(payload)?;
        let mut tx = self.transact_write()?;
        let res: JsonValue = tx
            .tx_attrs(attrs)?
            .iter()
            .map(|(op, aid)| json!([aid.0, op.to_string()]))
            .collect();
        let tx_id = tx.get_write_tx_id()?;
        tx.commit_tx(&comment, false)?;
        Ok(json!({
            "tx_id": tx_id,
            "results": res
        }))
    }
    pub fn current_schema(&self) -> Result<JsonValue> {
        let mut tx = self.transact()?;
        tx.all_attrs().map_ok(|v| v.to_json()).try_collect()
    }
    pub fn entities_at(&self, vld: &JsonValue) -> Result<JsonValue> {
        let vld = match vld {
            JsonValue::Null => Validity::current(),
            v => Validity::try_from(v)?,
        };
        let mut tx = self.transact()?;
        let mut current = encode_eav_key(
            EntityId::MIN_PERM,
            AttrId::MIN_PERM,
            &DataValue::Null,
            Validity::MAX,
        );
        let upper_bound = encode_eav_key(
            EntityId::MAX_PERM,
            AttrId::MAX_PERM,
            &DataValue::Bottom,
            Validity::MIN,
        );
        let mut it = tx
            .tx
            .iterator()
            .upper_bound(&upper_bound)
            .total_order_seek(true)
            .start();
        let mut collected: BTreeMap<EntityId, JsonValue> = BTreeMap::default();
        it.seek(&current);
        while let Some((k_slice, v_slice)) = it.pair()? {
            debug_assert_eq!(
                StorageTag::try_from(k_slice[0])?,
                StorageTag::TripleEntityAttrValue
            );
            let (e_found, a_found, vld_found) = decode_ea_key(k_slice)?;
            current.copy_from_slice(k_slice);

            if vld_found > vld {
                current.encoded_entity_amend_validity(vld);
                it.seek(&current);
                continue;
            }
            let op = StoreOp::try_from(v_slice[0])?;
            if op.is_retract() {
                current.encoded_entity_amend_validity_to_inf_past();
                it.seek(&current);
                continue;
            }
            let attr = tx.attr_by_id(a_found)?;
            if attr.is_none() {
                current.encoded_entity_amend_validity_to_inf_past();
                it.seek(&current);
                continue;
            }
            let attr = attr.unwrap();
            let value = if attr.cardinality.is_one() {
                decode_value_from_val(v_slice)?
            } else {
                decode_value_from_key(k_slice)?
            };
            let json_for_entry = collected.entry(e_found).or_insert_with(|| json!({}));
            let map_for_entry = json_for_entry.as_object_mut().unwrap();
            map_for_entry.insert("_id".to_string(), e_found.0.into());
            if attr.cardinality.is_many() {
                let arr = map_for_entry
                    .entry(attr.name.to_string())
                    .or_insert_with(|| json!([]));
                let arr = arr.as_array_mut().unwrap();
                arr.push(value.into());
            } else {
                map_for_entry.insert(attr.name.to_string(), value.into());
            }
            current.encoded_entity_amend_validity_to_inf_past();
            it.seek(&current);
        }
        let collected = collected.into_iter().map(|(_, v)| v).collect_vec();
        Ok(json!(collected))
    }
    pub fn run_script(&self, payload: &str) -> Result<JsonValue> {
        let payload = parse_query_to_json(payload)?;
        self.run_query(&payload)
    }
    pub fn explain_script(&self, payload: &str) -> Result<JsonValue> {
        let payload = parse_query_to_json(payload)?;
        self.explain_query(&payload)
    }
    pub fn run_query(&self, payload: &JsonValue) -> Result<JsonValue> {
        let mut tx = self.transact()?;
        let (input_program, out_opts, const_rules) =
            tx.parse_query(payload, &Default::default())?;
        let entry_head = &input_program.prog.get(&PROG_ENTRY).unwrap()[0].head.clone();
        let program = input_program
            .to_normalized_program()?
            .stratify()?
            .magic_sets_rewrite();
        debug!("{:#?}", program);
        let (compiled, mut stores) = tx.stratified_magic_compile(&program, &const_rules)?;
        let result = tx.stratified_magic_evaluate(
            &compiled,
            &mut stores,
            if out_opts.sorters.is_empty() {
                out_opts.num_to_take()
            } else {
                None
            },
        )?;
        if !out_opts.sorters.is_empty() {
            let sorted_result = tx.sort_and_collect(result, &out_opts.sorters, entry_head)?;
            let sorted_iter = if let Some(offset) = out_opts.offset {
                Left(sorted_result.scan_sorted().skip(offset))
            } else {
                Right(sorted_result.scan_sorted())
            };
            let sorted_iter = if let Some(limit) = out_opts.limit {
                Left(sorted_iter.take(limit))
            } else {
                Right(sorted_iter)
            };
            let ret: Vec<_> = tx
                .run_pull_on_query_results(sorted_iter, out_opts)?
                .try_collect()?;
            Ok(json!(ret))
        } else {
            let ret: Vec<_> = tx
                .run_pull_on_query_results(result.scan_all(), out_opts)?
                .try_collect()?;
            Ok(json!(ret))
        }
    }
    pub fn explain_query(&self, payload: &JsonValue) -> Result<JsonValue> {
        let mut tx = self.transact()?;
        let (input_program, _out_opts, const_rules) =
            tx.parse_query(payload, &Default::default())?;
        let normalized_program = input_program.to_normalized_program()?;
        let stratified_program = normalized_program.stratify()?;
        let magic_program = stratified_program.magic_sets_rewrite();
        let (_compiled_strata, _) = tx.stratified_magic_compile(&magic_program, &const_rules)?;

        todo!()
    }
}
