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
#else
	#error Unsupported variant
#endif
