import test from 'node:test';
import assert from 'node:assert/strict';
import { pageNumbers } from '../src/components/ui/paginationMath.ts';

/*
 * 页码序列的边界：少页全给、多页给两端与当前页附近、中间用省略号占位。
 * 这段逻辑在界面上只有一处实现，两个分页表共用，因此它的边界单独测。
 */

test('页数不超过七页时全部列出', () => {
    assert.deepEqual(pageNumbers(1, 1), [1]);
    assert.deepEqual(pageNumbers(3, 5), [1, 2, 3, 4, 5]);
    assert.deepEqual(pageNumbers(7, 7), [1, 2, 3, 4, 5, 6, 7]);
});

test('页数多时保留两端与当前页附近，中间以省略号占位', () => {
    assert.deepEqual(pageNumbers(1, 20), [1, 2, 'gap', 20]);
    assert.deepEqual(pageNumbers(10, 20), [1, 'gap', 9, 10, 11, 'gap', 20]);
    assert.deepEqual(pageNumbers(20, 20), [1, 'gap', 19, 20]);
});

test('页码始终升序、无重复、不越界', () => {
    for (const current of [1, 2, 3, 9, 10, 11, 19, 20]) {
        const pages = pageNumbers(current, 20).filter((item) => item !== 'gap');
        assert.deepEqual([...pages].sort((a, b) => a - b), pages);
        assert.equal(new Set(pages).size, pages.length);
        assert.ok(pages.every((page) => page >= 1 && page <= 20));
        assert.ok(pages.includes(current), '当前页必须在序列里');
    }
});
