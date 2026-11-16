#include <assert.h>
#include <stdint.h>
#include <string.h>
typedef uint8_t u8;
typedef uint32_t u32;
#include "library_dependencies.h"
static u32 header[GRKIT_HOST_HEADER_SIZE/4];
static u8 bytes[130];
int main(void)
{
    memcpy(bytes,"GRKLIB01",8); library_store_put(bytes+8,1); library_store_put(bytes+12,130);
    library_store_put(bytes+16,1); library_store_put(bytes+20,1);
    memcpy(bytes+32,"sample",6); memcpy(bytes+64,"Sample",6);
    library_store_put(bytes+116,8); library_store_put(bytes+24,library_store_crc(bytes,130,24,4));
    struct library_manifest manifest; struct library_dependencies out;
    assert(library_manifest_open(bytes,sizeof(bytes),&manifest));
    assert(library_dependencies_read(&manifest,header,&out) && !out.count);
    u32 code=library_dependency_code(bytes+32);
    u32 word=GRKIT_RECIPE_USED|GRKIT_RECIPE_HIDDEN|GRKIT_RECIPE_LOOK_EFFECT|(code<<GRKIT_RECIPE_CODE_SHIFT);
    header[GRKIT_HOST_RECIPES/4]=word; header[GRKIT_HOST_RECIPE_IDS/4]=42;
    header[GRKIT_HOST_RECIPES/4+GRKIT_RECIPE_MODULE_WORD]=GRKIT_RECIPE_OFF;
    u32 *dial=header+GRKIT_HOST_DIAL_SETS/4;
    dial[GRKIT_DIAL_SET_COUNT]=1; dial[GRKIT_DIAL_SET_CORE]=GRKIT_DIAL_SET_EFFECT|code;
    assert(library_dependencies_read(&manifest,header,&out) && out.count==2);
    assert(out.items[0].entry==0 && out.items[0].kind==LIBRARY_DEPENDENCY_RECIPE && out.items[0].id==42);
    assert(out.items[1].kind==LIBRARY_DEPENDENCY_DIAL && out.items[1].id==1);
    header[GRKIT_HOST_RECIPE_IDS/4]=0; assert(!library_dependencies_read(&manifest,header,&out));
    header[GRKIT_HOST_RECIPE_IDS/4]=42;
    header[GRKIT_HOST_RECIPES/4+2*GRKIT_RECIPE_WORDS]=word; assert(!library_dependencies_read(&manifest,header,&out));
    header[GRKIT_HOST_RECIPES/4+2*GRKIT_RECIPE_WORDS]=0;
    dial[GRKIT_DIAL_SET_COUNT]=GRKIT_DIAL_SET_PAIRS+2; assert(!library_dependencies_read(&manifest,header,&out));
    dial[GRKIT_DIAL_SET_COUNT]=1; dial[GRKIT_DIAL_SET_CORE]=code; assert(!library_dependencies_read(&manifest,header,&out));
    u8 collision[228]={0};
    memcpy(collision,"GRKLIB01",8); library_store_put(collision+8,1); library_store_put(collision+12,sizeof(collision));
    library_store_put(collision+16,2); library_store_put(collision+20,2);
    memcpy(collision+32,"look-1400",9); memcpy(collision+128,"look-61",7);
    memcpy(collision+64,"First",5); memcpy(collision+160,"Second",6); collision[226]=1;
    library_store_put(collision+24,library_store_crc(collision,sizeof(collision),24,4));
    assert(library_manifest_open(collision,sizeof(collision),&manifest));
    assert(library_dependency_code(collision+32)==library_dependency_code(collision+128));
    memset(header,0,sizeof(header)); header[GRKIT_HOST_RECIPE_IDS/4]=42;
    header[GRKIT_HOST_RECIPES/4]=GRKIT_RECIPE_USED|GRKIT_RECIPE_LOOK_EFFECT|(library_dependency_code(collision+32)<<GRKIT_RECIPE_CODE_SHIFT);
    assert(!library_dependencies_read(&manifest,header,&out));
    return 0;
}
