use sashiko::worker::prefetch::{extract_enclosing_block, parse_diff_ranges};

#[test]
fn test_tree_sitter_prefetch_e2e() {
    let source_code = r#"#include <linux/mm.h>

struct dmem_cgroup_pool {
    struct list_head list;
    unsigned long size;
};

static void dmem_cgroup_pool_free(struct dmem_cgroup_pool *pool)
{
    // A comment
    kfree(pool);
}

void dmem_cgroup_unregister_region(struct dmem_cgroup_region *region)
{
    struct dmem_cgroup_pool *pool, *next;
    
    mutex_lock(&dmem_cgroup_mutex);
    list_for_each_entry_safe(pool, next, &region->pools, list) {
        list_del(&pool->list);
        dmem_cgroup_pool_free(pool);
    }
    mutex_unlock(&dmem_cgroup_mutex);
}

int main() {
    return 0;
}
"#;

    let diff = r#"
--- a/mm/dmem.c
+++ b/mm/dmem.c
@@ -20,2 +20,3 @@
     list_for_each_entry_safe(pool, next, &region->pools, list) {
         list_del(&pool->list);
+        // We should delay this
         dmem_cgroup_pool_free(pool);
     }
"#;

    let ranges = parse_diff_ranges(diff);
    let file_ranges = ranges.get("mm/dmem.c").unwrap();

    let (start, end) = file_ranges[0];
    let block = extract_enclosing_block(source_code, start, end).unwrap();

    assert!(block.0.starts_with("void dmem_cgroup_unregister_region("));
    assert!(block.0.ends_with("mutex_unlock(&dmem_cgroup_mutex);\n}"));
}
