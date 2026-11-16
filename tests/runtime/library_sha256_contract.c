#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
typedef uint8_t u8;
typedef uint32_t u32;
#include "library_sha256.h"
int main(int argc,char **argv)
{
    static u8 bytes[LIBRARY_SHA256_MAX_BYTES+1];
    u8 digest[32];
    if(argc==2 || argc==3) {
        FILE *file=fopen(argv[1],"rb"); assert(file);
        size_t count=fread(bytes,1,sizeof(bytes),file); assert(!ferror(file)); fclose(file);
        if(argc==2) assert(count<=32768 && library_sha256(bytes,(u32)count,digest));
        else {
            struct library_sha256_state hash; library_sha256_init(&hash);
            u32 chunk=(u32)strtoul(argv[2],0,10); assert(chunk && chunk<=LIBRARY_SHA256_MAX_BYTES);
            for(u32 i=0;i<count;) {
                u32 n=(u32)count-i; if(n>chunk) n=chunk;
                assert(library_sha256_update(&hash,bytes+i,n)); i+=n;
            }
            assert(library_sha256_final(&hash,digest));
            u8 again[32]; assert(library_sha256_final(&hash,again) && !memcmp(digest,again,32));
        }
        for(u32 i=0;i<32;++i) printf("%02x",digest[i]);
        return 0;
    }
    assert(!library_sha256(bytes,32769,digest));
    assert(!library_sha256(0,1,digest));
    assert(!library_sha256(bytes,1,0));
    assert(library_sha256(0,0,digest));
    struct library_sha256_state hash; library_sha256_init(&hash);
    assert(library_sha256_update(&hash,bytes,LIBRARY_SHA256_MAX_BYTES));
    assert(!library_sha256_update(&hash,bytes,1));
    assert(library_sha256_final(&hash,digest));
    hash.used=65; assert(!library_sha256_final(&hash,digest));
    return 0;
}
