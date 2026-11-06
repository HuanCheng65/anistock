-- 016: 热度快讯 no longer show the hidden valuation. Their body used to end with
-- "估值随之修正：<old> → <new> 星辉。"; the published ones get the new wording.
UPDATE news
SET body = substr(body, 1, instr(body, '。') - 1) || '，与前几集同一时间相比。'
WHERE kind = 'flash' AND instr(body, '估值随之修正') > 0;
