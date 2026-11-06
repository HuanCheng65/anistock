// What 身份验证 unlocks (backend/src/kyc.rs and the credit steps): shown in 设置, on the account receipt and in
// 新手指南, so a new player knows why to bind QQ. Trading itself never needs it.
export const KYC_UNLOCKS: readonly [string, string][] = [
  ['财政补贴', '每周按人平分'],
  ['新股认购', '开播前按发行价认购'],
  ['排行榜', '收益率进入排名'],
  ['融资融券', '开通信用账户'],
  ['找回密码', '通过 QQ 机器人重置'],
]
