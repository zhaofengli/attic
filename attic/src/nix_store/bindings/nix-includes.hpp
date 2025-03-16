#if defined(ATTIC_VARIANT_NIX)
	#if NIX_VERSION >= 226
		#include <nix/config-main.hh>
		#include <nix/config-store.hh>
	#else
		#include <nix/config.h>
	#endif

	#include <nix/store-api.hh>
	#include <nix/local-store.hh>
	#include <nix/remote-store.hh>
	#include <nix/uds-remote-store.hh>
	#include <nix/hash.hh>
	#include <nix/path.hh>
	#include <nix/serialise.hh>
	#include <nix/shared.hh>
#elif defined(ATTIC_VARIANT_LIX)
	#include <lix/config.h>

	#include <lix/libstore/store-api.hh>
	#include <lix/libstore/local-store.hh>
	#include <lix/libstore/remote-store.hh>
	#include <lix/libstore/uds-remote-store.hh>
	#include <lix/libutil/hash.hh>
	#include <lix/libstore/path.hh>
	#include <lix/libutil/serialise.hh>
	#include <lix/libmain/shared.hh>
#else
	#error Unsupported variant
#endif
