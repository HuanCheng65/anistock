-- The index weights' free float (docs/api-v1.md §4.1.4): each listed company's shares not held by its production
-- committee or itself, recorded at every close. The indices weigh a month by the float recorded at the close before
-- its first market day.
ALTER TABLE company_days ADD COLUMN float_shares INTEGER;
