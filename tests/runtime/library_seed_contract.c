#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
typedef uint8_t u8;
typedef uint32_t u32;
#include "library_seed.h"
#include "grf.h"
static u8 manifest[LIBRARY_STORE_MANIFEST_MAX],grf[GRKIT_GRF_MAX_SIZE+1];
static int validate(const u8 *bytes,u32 length) { return grkit_grf_validate(bytes,length); }
static u32 read_grf(const char *folder,u32 index)
{
    char path[1024]; assert(snprintf(path,sizeof(path),"%s/%u.grf",folder,index)<(int)sizeof(path));
    FILE *file=fopen(path,"rb"); assert(file); u32 length=(u32)fread(grf,1,sizeof(grf),file); assert(!ferror(file)); fclose(file); return length;
}
int main(int argc,char **argv)
{
    assert(argc==3 || argc==4); u32 count=(u32)strtoul(argv[2],0,10),length=0;
    struct library_seed seed;
    assert(library_seed_begin(&seed,manifest,sizeof(manifest)));
    if(count) {
        u32 size=read_grf(argv[1],0); u8 wrong[32]={0};
        assert(!library_seed_add(&seed,wrong,grf,size,1,0,validate) && seed.error==LIBRARY_SEED_INVALID);
        assert(library_seed_begin(&seed,manifest,sizeof(manifest)));
        assert(library_seed_add(&seed,grf+GRKIT_GRF_ID,grf,size,1,0,validate));
        assert(!library_seed_add(&seed,grf+GRKIT_GRF_ID,grf,size,1,0,validate) && seed.error==LIBRARY_SEED_CONFLICT);
        assert(!library_seed_finish(&seed,&length));
        assert(library_seed_begin(&seed,manifest,sizeof(manifest)));
    }
    for(u32 i=0;i<count;++i) {
        u32 size=read_grf(argv[1],i);
        u32 enabled=(argc==4 && !strcmp(argv[3],"capacity"))?1U:i<40;
        int added=library_seed_add(&seed,grf+GRKIT_GRF_ID,grf,size,enabled,i?0:0x8001U,validate);
        if(!added && argc==4) {
            assert(seed.error==(!strcmp(argv[3],"capacity")?LIBRARY_SEED_CAPACITY:LIBRARY_SEED_CONFLICT));
            assert(!library_seed_finish(&seed,&length)); return 0;
        }
        assert(added);
    }
    assert(argc==3);
    assert(library_seed_finish(&seed,&length));
    u32 again=0; assert(library_seed_finish(&seed,&again) && again==length);
    assert(fwrite(manifest,1,length,stdout)==length);
    return 0;
}
