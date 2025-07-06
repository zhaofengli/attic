#if defined(ATTIC_VARIANT_NIX)
	#if NIX_VERSION >= 228
		#include <nix/store/store-api.hh>
		#include <nix/store/local-store.hh>
		#include <nix/store/remote-store.hh>
		#include <nix/store/uds-remote-store.hh>
		#include <nix/store/path.hh>
		#include <nix/util/hash.hh>
		#include <nix/util/serialise.hh>
		#include <nix/main/shared.hh>
	#else
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
	#endif
#else
	#error Unsupported variant
#endif
