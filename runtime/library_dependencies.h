#ifndef GRKIT_LIBRARY_DEPENDENCIES_H
#define GRKIT_LIBRARY_DEPENDENCIES_H
#include "abi.h"
#include "library_manifest.h"
#define LIBRARY_DEPENDENCY_RECIPE 1U
#define LIBRARY_DEPENDENCY_DIAL 2U
#define LIBRARY_DEPENDENCY_MAX (GRKIT_RECIPE_SLOTS+GRKIT_DIAL_USER_MODES)
struct library_dependency { u32 entry,kind,id; };
struct library_dependencies { u32 count; struct library_dependency items[LIBRARY_DEPENDENCY_MAX]; };
static u32 library_dependency_code(const u8 *id)
{
    u32 hash=0x811c9dc5U;
    for(u32 i=0;i<31 && id[i];++i) hash=(hash^id[i])*0x01000193U;
    return hash&GRKIT_RECIPE_CODE_MASK;
}
/* Return -1 for an absent identity, -2 for an ambiguous code. Ambiguity cannot
   be resolved by directory order: doing so could protect the wrong Look. */
static int library_dependency_entry(const struct library_manifest *manifest,u32 code)
{
    int found=-1;
    for(u32 i=0;i<manifest->entries;++i) {
        const u8 *record=library_manifest_record(manifest,i);
        if(library_dependency_code(record)==code) {
            if(found>=0) return -2;
            found=(int)i;
        }
    }
    return found;
}
static int library_dependency_add(struct library_dependencies *out,const struct library_manifest *manifest,u32 code,u32 kind,u32 id)
{
    int entry=library_dependency_entry(manifest,code);
    if(entry==-2) return 0;
    if(entry<0) return 1;
    if(out->count>=LIBRARY_DEPENDENCY_MAX) return 0;
    out->items[out->count++]=(struct library_dependency){(u32)entry,kind,id};
    return 1;
}
/* Call only in the App task while recipe/dial mutation is excluded. A successful
   scan includes disabled and hidden recipes and all saved dial sets. References
   to absent Looks remain absent; they must not attach to an arbitrary record.
   On failure discard the partial output and report dependencies incomplete. */
static int library_dependencies_read(const struct library_manifest *manifest,const volatile u32 *header,struct library_dependencies *out)
{
    if(!manifest || !header || !out) return 0;
    out->count=0;
    u32 ended=0;
    for(u32 i=0;i<GRKIT_RECIPE_SLOTS;++i) {
        u32 word=header[GRKIT_HOST_RECIPES/4+i*GRKIT_RECIPE_WORDS];
        if(!word) { ended=1; continue; }
        if(ended || !(word&GRKIT_RECIPE_USED) || (word>>GRKIT_RECIPE_VALUE_BITS) ||
           (word&GRKIT_RECIPE_LOOK_MASK)>GRKIT_RECIPE_LOOK_EFFECT ||
           ((word>>GRKIT_RECIPE_GRAIN_LEVEL_SHIFT)&GRKIT_RECIPE_GRAIN_LEVEL_MASK)>=GRKIT_CORE_GRAIN_VALUES) return 0;
        if((word&GRKIT_RECIPE_LOOK_MASK)==GRKIT_RECIPE_LOOK_EFFECT) {
            u32 id=header[GRKIT_HOST_RECIPE_IDS/4+i];
            if(!id || id>GRKIT_RECIPE_ID_MAX) return 0;
            for(u32 j=0;j<i;++j) if(header[GRKIT_HOST_RECIPE_IDS/4+j]==id) return 0;
            if(!library_dependency_add(out,manifest,(word>>GRKIT_RECIPE_CODE_SHIFT)&GRKIT_RECIPE_CODE_MASK,LIBRARY_DEPENDENCY_RECIPE,id)) return 0;
        }
    }
    for(u32 i=0;i<GRKIT_DIAL_USER_MODES;++i) {
        const volatile u32 *set=header+GRKIT_HOST_DIAL_SETS/4+i*GRKIT_DIAL_SET_WORDS;
        u32 count=set[GRKIT_DIAL_SET_COUNT];
        if(!count) continue;
        if(count>GRKIT_DIAL_SET_PAIRS+1U) return 0;
        u32 effect=set[GRKIT_DIAL_SET_CORE+GRKIT_CORE_TONE-GRKIT_CORE_SETTING_BASE];
        if(!effect) continue;
        if(!(effect&GRKIT_DIAL_SET_EFFECT) || (effect&~(GRKIT_DIAL_SET_EFFECT|GRKIT_RECIPE_CODE_MASK))) return 0;
        if(!library_dependency_add(out,manifest,effect&GRKIT_RECIPE_CODE_MASK,LIBRARY_DEPENDENCY_DIAL,i+1)) return 0;
    }
    return 1;
}
#endif
