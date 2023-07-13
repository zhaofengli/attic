// C++ side of the libnixstore glue.
//
// We implement a mid-level wrapper of the Nix Store interface,
// which is then wrapped again in the Rust side to enable full
// async-await operation.
//
// Here we stick with the naming conventions of Rust and handle
// Rust types directly where possible, so that the interfaces are
// satisfying to use from the Rust side via cxx.rs.

#include "attic/src/nix_store/bindings/nix.hpp"

static std::mutex g_init_nix_mutex;
static bool g_init_nix_done = false;

static nix::StorePath store_path_from_rust(RBasePathSlice base_name) {
	std::string_view sv((const char *)base_name.data(), base_name.size());
	return nix::StorePath(sv);
}

// ========
// RustSink
// ========

RustSink::RustSink(RBox<AsyncWriteSender> sender) : sender(std::move(sender)) {}

void RustSink::operator () (std::string_view data) {
	RBasePathSlice s((const unsigned char *)data.data(), data.size());

	this->sender->send(s);
}

void RustSink::eof() {
	this->sender->eof();
}


// =========
// CPathInfo
// =========

CPathInfo::CPathInfo(nix::ref<const nix::ValidPathInfo> pi) : pi(pi) {}

RHashSlice CPathInfo::nar_sha256_hash() {
	auto &hash = this->pi->narHash;

	if (hash.type != nix::htSHA256) {
		throw nix::Error("Only SHA-256 hashes are supported at the moment");
	}

	return RHashSlice(hash.hash, hash.hashSize);
}

uint64_t CPathInfo::nar_size() {
	return this->pi->narSize;
}

std::unique_ptr<std::vector<std::string>> CPathInfo::sigs() {
	std::vector<std::string> result;
	for (auto&& elem : this->pi->sigs) {
		result.push_back(std::string(elem));
	}
	return std::make_unique<std::vector<std::string>>(result);
}

std::unique_ptr<std::vector<std::string>> CPathInfo::references() {
	std::vector<std::string> result;
	for (auto&& elem : this->pi->references) {
		result.push_back(std::string(elem.to_string()));
	}
	return std::make_unique<std::vector<std::string>>(result);
}

RString CPathInfo::ca() {
	if (this->pi->ca) {
		return RString(nix::renderContentAddress(this->pi->ca));
	} else {
		return RString("");
	}
}

// =========
// CNixStore
// =========

CNixStore::CNixStore() {
	std::map<std::string, std::string> params;
	std::lock_guard<std::mutex> lock(g_init_nix_mutex);

	if (!g_init_nix_done) {
		nix::initNix();
		g_init_nix_done = true;
	}

	this->store = nix::openStore(nix::settings.storeUri.get(), params);
}

RString CNixStore::store_dir() {
	return RString(this->store->storeDir);
}

std::unique_ptr<CPathInfo> CNixStore::query_path_info(RBasePathSlice base_name) {
	auto store_path = store_path_from_rust(base_name);

	auto r = this->store->queryPathInfo(store_path);
	return std::make_unique<CPathInfo>(r);
}

std::unique_ptr<std::vector<std::string>> CNixStore::compute_fs_closure(RBasePathSlice base_name, bool flip_direction, bool include_outputs, bool include_derivers) {
	std::set<nix::StorePath> out;

	this->store->computeFSClosure(store_path_from_rust(base_name), out, flip_direction, include_outputs, include_derivers);

	std::vector<std::string> result;
	for (auto&& elem : out) {
		result.push_back(std::string(elem.to_string()));
	}
	return std::make_unique<std::vector<std::string>>(result);
}

std::unique_ptr<std::vector<std::string>> CNixStore::compute_fs_closure_multi(RSlice<const RBasePathSlice> base_names, bool flip_direction, bool include_outputs, bool include_derivers) {
	std::set<nix::StorePath> path_set, out;
	for (auto&& base_name : base_names) {
		path_set.insert(store_path_from_rust(base_name));
	}

	this->store->computeFSClosure(path_set, out, flip_direction, include_outputs, include_derivers);

	std::vector<std::string> result;
	for (auto&& elem : out) {
		result.push_back(std::string(elem.to_string()));
	}
	return std::make_unique<std::vector<std::string>>(result);
}

void CNixStore::nar_from_path(RVec<unsigned char> base_name, RBox<AsyncWriteSender> sender) {
	RustSink sink(std::move(sender));

	std::string_view sv((const char *)base_name.data(), base_name.size());
	nix::StorePath store_path(sv);

	// exceptions will be thrown into Rust
	this->store->narFromPath(store_path, sink);
	sink.eof();
}

std::unique_ptr<CNixStore> open_nix_store() {
	return std::make_unique<CNixStore>();
}
