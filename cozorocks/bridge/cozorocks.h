//
// Created by Ziyang Hu on 2022/4/13.
//

#pragma once

#include <memory>
#include<iostream>
#include <shared_mutex>
#include "rust/cxx.h"

#include "rocksdb/db.h"
#include "rocksdb/slice.h"
#include "rocksdb/options.h"
#include "rocksdb/utilities/transaction.h"
#include "rocksdb/utilities/transaction_db.h"
#include "rocksdb/utilities/optimistic_transaction_db.h"
#include "rocksdb/table.h"
#include "rocksdb/filter_policy.h"
#include "rocksdb/slice_transform.h"


typedef std::shared_mutex Lock;
typedef std::unique_lock<Lock> WriteLock;
typedef std::shared_lock<Lock> ReadLock;


using namespace ROCKSDB_NAMESPACE;
using std::unique_ptr;
using std::shared_ptr;
using std::make_unique;
using std::make_shared;
using std::string;
using std::vector;
using std::unordered_map;
using std::tuple;

struct BridgeStatus;

typedef Status::Code StatusCode;
typedef Status::SubCode StatusSubCode;
typedef Status::Severity StatusSeverity;

inline shared_ptr<Slice> make_shared_slice(unique_ptr<Slice> s) {
    shared_ptr<Slice> ret = std::move(s);
    return ret;
}

inline shared_ptr<PinnableSlice> make_shared_pinnable_slice(unique_ptr<PinnableSlice> s) {
    shared_ptr<PinnableSlice> ret = std::move(s);
    return ret;
}

inline shared_ptr<Options> make_shared_options(unique_ptr<Options> o) {
    shared_ptr<Options> ret = std::move(o);
    return ret;
}

inline Slice convert_slice(rust::Slice<const uint8_t> d) {
    return Slice(reinterpret_cast<const char *>(d.data()), d.size());
}

inline rust::Slice<const uint8_t> convert_slice_back(const Slice &s) {
    return rust::Slice(reinterpret_cast<const std::uint8_t *>(s.data()), s.size());
}


inline rust::Slice<const uint8_t> convert_pinnable_slice_back(const PinnableSlice &s) {
    return rust::Slice(reinterpret_cast<const std::uint8_t *>(s.data()), s.size());
}

void write_status_impl(BridgeStatus &status, StatusCode code, StatusSubCode subcode, StatusSeverity severity,
                       int bridge_code);

inline void write_status(Status &&rstatus, BridgeStatus &status) {
    if (rstatus.code() != StatusCode::kOk || rstatus.subcode() != StatusSubCode::kNoSpace ||
        rstatus.severity() != StatusSeverity::kNoError) {
        write_status_impl(status, rstatus.code(), rstatus.subcode(), rstatus.severity(), 0);
    }
}

void set_verify_checksums(ReadOptions &options, const bool v) {
    options.verify_checksums = v;
}

void set_total_order_seek(ReadOptions &options, const bool v) {
    options.total_order_seek = v;
}

void set_prefix_same_as_start(ReadOptions &options, const bool v) {
    options.prefix_same_as_start = v;
}

void set_auto_prefix_mode(ReadOptions &options, const bool v) {
    options.auto_prefix_mode = v;
}

void set_disable_wal(WriteOptions &options, const bool v) {
    options.disableWAL = v;
}


typedef rust::Fn<std::int8_t(rust::Slice<const std::uint8_t>, rust::Slice<const std::uint8_t>)> RustComparatorFn;

class RustComparator : public Comparator {
public:
    inline int Compare(const rocksdb::Slice &a, const rocksdb::Slice &b) const {
        return int(rust_compare(convert_slice_back(a), convert_slice_back(b)));
    }

    const char *Name() const {
        return name.c_str();
    }

    virtual bool CanKeysWithDifferentByteContentsBeEqual() const {
        return can_different_bytes_be_equal;
    }

    void FindShortestSeparator(std::string *, const rocksdb::Slice &) const {}

    void FindShortSuccessor(std::string *) const {}

    void set_fn(RustComparatorFn f) {
        rust_compare = f;
    }

    void set_name(rust::Str name_) {
        name = std::string(name_);
    }

    void set_can_different_bytes_be_equal(bool v) {
        can_different_bytes_be_equal = v;
    }

    std::string name;
    RustComparatorFn rust_compare;
    bool can_different_bytes_be_equal;
};

inline unique_ptr<RustComparator> new_rust_comparator(rust::Str name, RustComparatorFn f, bool diff_bytes_can_equal) {
    auto ret = make_unique<RustComparator>();
    ret->set_name(name);
    ret->set_fn(f);
    ret->set_can_different_bytes_be_equal(diff_bytes_can_equal);
    return ret;
}


inline void prepare_for_bulk_load(Options &inner) {
    inner.PrepareForBulkLoad();
}

inline void increase_parallelism(Options &inner, uint32_t size) {
    inner.IncreaseParallelism(size);
}

inline void optimize_level_style_compaction(Options &inner) {
    inner.OptimizeLevelStyleCompaction();
};

inline void set_create_if_missing(Options &inner, bool v) {
    inner.create_if_missing = v;
}

inline void set_comparator(Options &inner, const RustComparator &cmp_obj) {
    inner.comparator = &cmp_obj;
}

inline void set_paranoid_checks(Options &inner, bool v) {
    inner.paranoid_checks = v;
}

inline void set_enable_blob_files(Options &inner, bool v) {
    inner.enable_blob_files = v;
}

inline void set_min_blob_size(Options &inner, uint64_t size) {
    inner.min_blob_size = size;
}

inline void set_blob_file_size(Options &inner, uint64_t size) {
    inner.blob_file_size = size;
}

inline void set_enable_blob_garbage_collection(Options &inner, bool v) {
    inner.enable_blob_garbage_collection = v;
}

// TODO: more API for Options
// blob_compression_type,
// blob_garbage_collection_age_cutoff,
// blob_garbage_collection_force_threshold,
// blob_compaction_readahead_size


inline std::unique_ptr<ReadOptions> new_read_options() {
    return std::make_unique<ReadOptions>();
}

inline std::unique_ptr<WriteOptions> new_write_options() {
    return std::make_unique<WriteOptions>();
}

inline std::unique_ptr<Options> new_options() {
    return std::make_unique<Options>();
}

inline void set_bloom_filter(Options &options, const double bits_per_key, const bool whole_key_filtering) {
    BlockBasedTableOptions table_options;
    table_options.filter_policy.reset(NewBloomFilterPolicy(bits_per_key, false));
    table_options.whole_key_filtering = whole_key_filtering;
    options.table_factory.reset(
            NewBlockBasedTableFactory(
                    table_options));
}

inline void set_capped_prefix_extractor(Options &options, const size_t cap_len) {
    options.prefix_extractor.reset(NewCappedPrefixTransform(cap_len));
}

inline void set_fixed_prefix_extractor(Options &options, const size_t prefix_len) {
    options.prefix_extractor.reset(NewFixedPrefixTransform(prefix_len));
}

struct IteratorBridge {
    mutable std::unique_ptr<Iterator> inner;

    IteratorBridge(Iterator *it) : inner(it) {}

    inline void seek_to_first() const {
        inner->SeekToFirst();
    }

    inline void seek_to_last() const {
        inner->SeekToLast();
    }

    inline void next() const {
        inner->Next();
    }

    inline bool is_valid() const {
        return inner->Valid();
    }

    inline void do_seek(rust::Slice<const uint8_t> key) const {
        auto k = Slice(reinterpret_cast<const char *>(key.data()), key.size());
        inner->Seek(k);
    }

    inline void do_seek_for_prev(rust::Slice<const uint8_t> key) const {
        auto k = Slice(reinterpret_cast<const char *>(key.data()), key.size());
        inner->SeekForPrev(k);
    }

    inline std::unique_ptr<Slice> key_raw() const {
        return std::make_unique<Slice>(inner->key());
    }

    inline std::unique_ptr<Slice> value_raw() const {
        return std::make_unique<Slice>(inner->value());
    }

    inline void refresh(BridgeStatus &status) const {
        write_status(inner->Refresh(), status);
    }

    BridgeStatus status() const;
};


inline unique_ptr<TransactionOptions> new_transaction_options() {
    return make_unique<TransactionOptions>();
}

inline void set_deadlock_detect(TransactionOptions &inner, bool v) {
    inner.deadlock_detect = v;
}

inline unique_ptr<OptimisticTransactionOptions> new_optimistic_transaction_options(const RustComparator &compare) {
    auto ret = make_unique<OptimisticTransactionOptions>();
    ret->cmp = &compare;
    return ret;
}

inline void reset_pinnable_slice(PinnableSlice &slice) {
    slice.Reset();
}

unique_ptr<PinnableSlice> new_pinnable_slice() {
    return make_unique<PinnableSlice>();
}

struct TransactionBridge {
    DB *raw_db;
    unique_ptr<Transaction> inner;
    mutable unique_ptr<TransactionOptions> t_ops; // Put here to make sure ownership works
    mutable unique_ptr<OptimisticTransactionOptions> o_ops; // same as above
//    mutable unique_ptr<ReadOptions> r_ops;
//    mutable unique_ptr<ReadOptions> raw_r_ops;
    mutable unique_ptr<WriteOptions> w_ops;
//    mutable unique_ptr<WriteOptions> raw_w_ops;

    inline void set_snapshot() const {
        inner->SetSnapshot();
    }

    inline bool set_readoption_snapshot_to_current(ReadOptions &read_opts) const {
        read_opts.snapshot = inner->GetSnapshot();
        return read_opts.snapshot != nullptr;
    }

    inline void commit(BridgeStatus &status) const {
        write_status(inner->Commit(), status);
//        r_ops->snapshot = nullptr;
    }

    inline void rollback(BridgeStatus &status) const {
        write_status(inner->Rollback(), status);
    }

    inline void set_savepoint() const {
        inner->SetSavePoint();
    }

    inline void rollback_to_savepoint(BridgeStatus &status) const {
        write_status(inner->RollbackToSavePoint(), status);
    }

    inline void pop_savepoint(BridgeStatus &status) const {
        write_status(inner->PopSavePoint(), status);
    }

    inline void get_txn(
            const ReadOptions &r_ops,
            rust::Slice<const uint8_t> key,
            PinnableSlice &pinnable_val,
            BridgeStatus &status
    ) const {
        write_status(inner->Get(r_ops, convert_slice(key), &pinnable_val), status);
    }

    inline void get_for_update_txn(
            const ReadOptions &r_ops,
            rust::Slice<const uint8_t> key,
            PinnableSlice &pinnable_val,
            BridgeStatus &status
    ) const {
        write_status(
                inner->GetForUpdate(r_ops,
                                    raw_db->DefaultColumnFamily(),
                                    convert_slice(key),
                                    &pinnable_val),
                status
        );
    }


    inline void put_txn(
            rust::Slice<const uint8_t> key,
            rust::Slice<const uint8_t> val,
            BridgeStatus &status
    ) const {
        write_status(inner->Put(convert_slice(key), convert_slice(val)), status);
    }

    inline void del_txn(
            rust::Slice<const uint8_t> key,
            BridgeStatus &status
    ) const {
        write_status(inner->Delete(convert_slice(key)), status);
    }

    inline std::unique_ptr<IteratorBridge> iterator_txn(const ReadOptions &r_ops) const {
        return std::make_unique<IteratorBridge>(
                inner->GetIterator(r_ops));
    }
};


struct TDBBridge {
    mutable unique_ptr<DB> db;
    mutable TransactionDB *tdb;
    mutable OptimisticTransactionDB *odb;
    bool is_odb;

    TDBBridge(DB *db_,
              TransactionDB *tdb_,
              OptimisticTransactionDB *odb_) :
            db(db_), tdb(tdb_), odb(odb_) {
        is_odb = (tdb_ == nullptr);
    }

    inline shared_ptr<TransactionBridge> begin_t_transaction(
            unique_ptr<WriteOptions> w_ops,
//            unique_ptr<WriteOptions> raw_w_ops,
//            unique_ptr<ReadOptions> r_ops,
//            unique_ptr<ReadOptions> raw_r_ops,
            unique_ptr<TransactionOptions> txn_options) const {
        if (tdb == nullptr) {
            return unique_ptr<TransactionBridge>(nullptr);
        }
        auto ret = make_shared<TransactionBridge>();
        ret->raw_db = tdb;
//        ret->r_ops = std::move(r_ops);
        ret->w_ops = std::move(w_ops);
//        ret->raw_r_ops = std::move(raw_r_ops);
//        ret->raw_w_ops = std::move(raw_w_ops);
        ret->t_ops = std::move(txn_options);
        Transaction *txn = tdb->BeginTransaction(*ret->w_ops, *ret->t_ops);
        ret->inner = unique_ptr<Transaction>(txn);
        return ret;
    }

    inline shared_ptr<TransactionBridge> begin_o_transaction(
            unique_ptr<WriteOptions> w_ops,
//            unique_ptr<WriteOptions> raw_w_ops,
//            unique_ptr<ReadOptions> r_ops,
//            unique_ptr<ReadOptions> raw_r_ops,
            unique_ptr<OptimisticTransactionOptions> txn_options) const {
        if (odb == nullptr) {
            return unique_ptr<TransactionBridge>(nullptr);
        }
        auto ret = make_shared<TransactionBridge>();
        ret->raw_db = odb;
//        ret->r_ops = std::move(r_ops);
        ret->w_ops = std::move(w_ops);
//        ret->raw_r_ops = std::move(raw_r_ops);
//        ret->raw_w_ops = std::move(raw_w_ops);
        ret->o_ops = std::move(txn_options);
        Transaction *txn = odb->BeginTransaction(*ret->w_ops, *ret->o_ops);
        ret->inner = unique_ptr<Transaction>(txn);
        return ret;
    }

    inline void close_raw(BridgeStatus &status) const {
        write_status(db->Close(), status);
    }

    inline void get_approximate_sizes_raw(
            rust::Slice<const rust::Slice<const uint8_t>> ranges,
            rust::Slice<uint64_t> sizes,
            BridgeStatus &status) const {
        uint64_t n = sizes.size();
        vector<Range> cpp_ranges;
        cpp_ranges.reserve(n);
        for (uint64_t i = 0; i < n; ++i) {
            auto x = ranges.at(2 * i);
            auto start = convert_slice(x);
            auto end = convert_slice(ranges.at(2 * i + 1));
            auto rg = Range(start, end);
            cpp_ranges.emplace_back(rg);
        };
        write_status(
                db->GetApproximateSizes(db->DefaultColumnFamily(),
                                        cpp_ranges.data(), (int) n, sizes.data()),
                status
        );
    }

    inline void del_range_raw(
            const WriteOptions &raw_w_ops,
            rust::Slice<const uint8_t> start_key,
            rust::Slice<const uint8_t> end_key,
            BridgeStatus &status
    ) const {
        write_status(
                db->GetRootDB()->DeleteRange(
                        raw_w_ops,
                        db->DefaultColumnFamily(),
                        convert_slice(start_key), convert_slice(end_key)),
                status);
    }

    inline void flush_raw(const FlushOptions &options, BridgeStatus &status) const {
        write_status(db->Flush(options), status);
    }

    inline void compact_all_raw(BridgeStatus &status) const {
        auto options = CompactRangeOptions();
        options.change_level = true;
        options.target_level = 0;
        options.exclusive_manual_compaction = false;
        write_status(db->CompactRange(options,
                                      db->DefaultColumnFamily(),
                                      nullptr, nullptr), status);
    }


    inline void get_raw(
            const ReadOptions &r_ops,
            rust::Slice<const uint8_t> key,
            PinnableSlice &pinnable_val,
            BridgeStatus &status
    ) const {
        write_status(
                db->Get(r_ops,
                        db->DefaultColumnFamily(),
                        convert_slice(key),
                        &pinnable_val),
                status
        );
    }

    inline void put_raw(
            const WriteOptions &raw_w_ops,
            rust::Slice<const uint8_t> key,
            rust::Slice<const uint8_t> val,
            BridgeStatus &status
    ) const {
        auto k = convert_slice(key);
        auto v = convert_slice(val);
        write_status(db->Put(raw_w_ops, k, v), status);
    }

    inline void del_raw(
            const WriteOptions &raw_w_ops,
            rust::Slice<const uint8_t> key,
            BridgeStatus &status
    ) const {
        write_status(db->Delete(raw_w_ops, convert_slice(key)), status);
    }

    inline std::unique_ptr<IteratorBridge> iterator_raw(const ReadOptions &raw_r_ops) const {
        return std::make_unique<IteratorBridge>(
                db->NewIterator(raw_r_ops));
    }
};

inline shared_ptr<TransactionDBOptions> new_tdb_options() {
    return make_shared<TransactionDBOptions>();
}

inline shared_ptr<OptimisticTransactionDBOptions> new_odb_options() {
    return make_shared<OptimisticTransactionDBOptions>();
}

inline unique_ptr<FlushOptions> new_flush_options() {
    return make_unique<FlushOptions>();
}

void set_flush_wait(FlushOptions &options, bool v) {
    options.wait = v;
}

void set_allow_write_stall(FlushOptions &options, bool v) {
    options.allow_write_stall = v;
}

inline shared_ptr<TDBBridge>
open_tdb_raw(const Options &options,
             const TransactionDBOptions &txn_db_options,
             const string &path,
             BridgeStatus &status) {
    std::vector<ColumnFamilyHandle *> handles;
    TransactionDB *txn_db = nullptr;

    write_status(TransactionDB::Open(options, txn_db_options, path, &txn_db), status);

    return make_shared<TDBBridge>(txn_db, txn_db, nullptr);
}


inline shared_ptr<TDBBridge>
open_odb_raw(const Options &options, const string &path, BridgeStatus &status) {
    OptimisticTransactionDB *txn_db = nullptr;

    write_status(OptimisticTransactionDB::Open(options,
                                               path,
                                               &txn_db), status);


    unordered_map<string, shared_ptr<ColumnFamilyHandle>> handle_map;

    return make_shared<TDBBridge>(txn_db, nullptr, txn_db);
}

inline shared_ptr<TDBBridge>
open_db_raw(const Options &options, const string &path, BridgeStatus &status) {
    DB *db = nullptr;

    write_status(DB::Open(options,
                          path,
                          &db), status);

    unordered_map<string, shared_ptr<ColumnFamilyHandle>> handle_map;

    return make_shared<TDBBridge>(db, nullptr, nullptr);
}

inline void repair_db_raw(const Options &options, const string &path, BridgeStatus &status) {
    write_status(RepairDB(path, options), status);
}

inline void destroy_db_raw(const Options &options, const string &path, BridgeStatus &status) {
    write_status(DestroyDB(path, options), status);
}