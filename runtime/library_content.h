#ifndef GRKIT_LIBRARY_CONTENT_H
#define GRKIT_LIBRARY_CONTENT_H
#include "abi.h"
#include "library_manifest.h"
#include "library_sha256.h"
/* Validate bytes read through the resident owner against a previously validated
   manifest entry. The shared GRF validator remains the source of format rules.
   Neither a matching filename nor a caller-supplied hash establishes identity. */
static int library_content_matches(const struct library_manifest_entry *entry,
                                  const u8 *bytes,u32 length,
                                  int (*validate_grf)(const u8 *,u32))
{
    if(!entry || !bytes || !validate_grf || (entry->flags&2U) ||
       (length!=GRKIT_GRF_SIZE && length!=GRKIT_GRF_MAX_SIZE) ||
       !validate_grf(bytes,length)) return 0;
    if(!library_store_equal(entry->id,bytes+GRKIT_GRF_ID,32) ||
       !library_store_equal(entry->name,bytes+GRKIT_GRF_NAME,20) ||
       entry->features!=library_store_word(bytes+GRKIT_GRF_FEATURES) ||
       ((entry->flags>>2)&1U)!=library_store_word(bytes+GRKIT_GRF_FLAGS)) return 0;
    u8 digest[32];
    return library_sha256(bytes,length,digest) && library_store_equal(entry->hash,digest,32);
}
#endif
