#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
typedef uint8_t u8;
typedef uint32_t u32;
#include "library_snapshot_json.h"
static u8 bytes[LIBRARY_STORE_MANIFEST_MAX];
static int emit(void *context,const u8 *text,u32 length)
{ return fwrite(text,1,length,(FILE *)context)==length; }
static int reject(void *context,const u8 *text,u32 length)
{ (void)context; (void)text; (void)length; return 0; }
struct number_output { char bytes[11]; u32 count; };
static int number_emit(void *context,const u8 *text,u32 length)
{
    struct number_output *out=context;
    if(length>10-out->count) return 0;
    memcpy(out->bytes+out->count,text,length); out->count+=length;
    return 1;
}
static void number_parity(u32 number)
{
    struct number_output out={{0},0};
    struct library_json_sink sink={&out,number_emit,0,0};
    char expected[11];
    int length=snprintf(expected,sizeof(expected),"%u",(unsigned)number);
    library_json_number(&sink,number);
    assert(!sink.failed && sink.count==(u32)length && out.count==(u32)length);
    assert(!strcmp(out.bytes,expected));
}
static void numbers(void)
{
    static const u32 edges[]={0,9,10,99,100,999,1000,999999999U,1000000000U,4294967295U};
    for(u32 i=0;i<sizeof(edges)/sizeof(edges[0]);++i) number_parity(edges[i]);
    u32 random=0x7a13b49fU;
    for(u32 i=0;i<10000;++i) {
        random=random*1664525U+1013904223U;
        number_parity(random);
    }
}
int main(int argc,char **argv)
{
    numbers();
    assert(argc==2);
    FILE *file=fopen(argv[1],"rb"); assert(file);
    u32 length=(u32)fread(bytes,1,sizeof(bytes),file); assert(!ferror(file)); fclose(file);
    struct library_manifest manifest; assert(library_manifest_open(bytes,length,&manifest));
    u32 loaded[40],count=0;
    for(u32 i=0;i<manifest.entries;++i) if(library_store_word(library_manifest_record(&manifest,i)+84)&1U) loaded[count++]=i;
    struct library_dependencies dependencies={0};
    if(manifest.listed) dependencies.items[dependencies.count++]=(struct library_dependency){library_manifest_order(&manifest,0),LIBRARY_DEPENDENCY_RECIPE,42};
    if(manifest.listed>1) dependencies.items[dependencies.count++]=(struct library_dependency){library_manifest_order(&manifest,1),LIBRARY_DEPENDENCY_DIAL,1};
    struct library_snapshot_official official={0}; u32 official_count=0;
    for(u32 i=0;i<manifest.entries;++i) {
        const u8 *entry=library_manifest_record(&manifest,i);
        if(((library_store_word(entry+84)>>3)&3U)==2) { official.id=entry; official.hash=entry+52; official_count=1; break; }
    }
    u32 reserved[]={256,257};
    struct library_snapshot_metadata meta={0};
    meta.manifest=&manifest; meta.card_id="card-test"; meta.epoch[0]=1; meta.epoch[1]=2; meta.epoch[2]=3; meta.epoch[3]=4;
    meta.generation=7; meta.loaded=loaded; meta.loaded_count=count; meta.selected=count?loaded[0]:256;
    meta.dependencies=&dependencies; meta.dependencies_complete=1;
    meta.reserved_codes=reserved; meta.reserved_count=2; meta.supported_features=0xffffffffU;
    meta.official=&official; meta.official_count=official_count;
    struct library_json_sink sink={0,reject,0,0};
    assert(!library_snapshot_json(&meta,&sink));
    sink.context=stdout; sink.write=emit;
    meta.hot_reload=2;
    assert(!library_snapshot_json(&meta,&sink));
    meta.hot_reload=manifest.listed!=0;
    assert(library_snapshot_json(&meta,&sink));
    return 0;
}
