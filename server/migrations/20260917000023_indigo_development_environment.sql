-- Replace the previous default Development color while retaining custom colors.
UPDATE environments
SET color = '#4f46e5'
WHERE slug = 'development' AND color = '#10b981';
