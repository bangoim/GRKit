#ifndef GRKIT_LIBRARY_ACTIVATION_H
#define GRKIT_LIBRARY_ACTIVATION_H
#include "library_store.h"
#define LIBRARY_ACTIVATION_BYTES 32U
/* LIB.ACT is written only after generation 1 and its contents have durable
   readback. The checksum binds the complete seed manifest payload. Later
   generations retain this marker; it is not a per-commit journal. */
static void library_activation_encode(u8 out[LIBRARY_ACTIVATION_BYTES],u32 seed_checksum)
{
    static const u8 magic[8]={'G','R','K','A','C','T','0','1'};
    for(u32 i=0;i<LIBRARY_ACTIVATION_BYTES;++i) out[i]=i<8 ? magic[i] : 0;
    library_store_put(out+8,1); library_store_put(out+12,1);
    library_store_put(out+16,seed_checksum);
    library_store_put(out+24,library_store_crc(out,LIBRARY_ACTIVATION_BYTES,24,4));
}
static int library_activation_validate(const u8 *bytes,u32 length,u32 *seed_checksum)
{
    static const u8 magic[8]={'G','R','K','A','C','T','0','1'};
    if(!bytes || !seed_checksum || length!=LIBRARY_ACTIVATION_BYTES ||
       !library_store_equal(bytes,magic,8) || library_store_word(bytes+8)!=1 ||
       library_store_word(bytes+12)!=1 || library_store_word(bytes+20) || library_store_word(bytes+28) ||
       library_store_word(bytes+24)!=library_store_crc(bytes,length,24,4)) return 0;
    *seed_checksum=library_store_word(bytes+16);
    return 1;
}
#endif
