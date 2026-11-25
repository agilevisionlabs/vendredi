WITH sources AS (
 SELECT t.id, CASE WHEN t.resend_of IS NULL THEN t.id ELSE (
   SELECT min(original.id) FROM transfers original
   WHERE original.root=t.root AND original.direction='out'
     AND original.resend_of IS NULL AND original.bytes=t.bytes
 ) END AS source FROM transfers t
), envelopes AS (
 SELECT transfer, group_concat(reference, ', ') AS refs,
        max(ack_status IN ('rejected','partial')) AS rejected, max(ack_status='accepted') AS accepted,
        max(ack_requested='1') AS requested
 FROM interchanges GROUP BY transfer
), types AS (
 SELECT i.transfer, group_concat(m.type, ', ') AS kinds,
        min(CASE WHEN m.release='' THEN m.version ELSE m.version || '.' || m.release END) AS version
 FROM messages m JOIN interchanges i ON m.interchange=i.id GROUP BY i.transfer
), errors AS (
 SELECT transfer, count(*) AS amount FROM findings GROUP BY transfer
), decisions AS (
 SELECT json_extract(body,'$.root') AS root,
        max(json_extract(body,'$.event')='no_match') AS missed,
        max(json_extract(body,'$.event')='validated' AND json_extract(body,'$.accepted')=0) AS rejected,
        max(json_extract(body,'$.event')='action' AND json_type(body,'$.error')='text') AS failed
 FROM events GROUP BY json_extract(body,'$.root')
)
SELECT t.id,t.root,t.time,t.direction,t.partner,t.status,
       coalesce(m.kinds,''),coalesce(e.refs,''),coalesce(m.version,''),
       coalesce(f.amount,0), coalesce(d.missed,0), coalesce(d.failed,0),
       coalesce(d.rejected,0),coalesce(e.rejected,0),coalesce(e.accepted,0),coalesce(e.requested,0),
       CASE WHEN ?1 THEN t.bytes END
FROM transfers t
LEFT JOIN sources s ON s.id=t.id
LEFT JOIN envelopes e ON e.transfer=s.source
LEFT JOIN types m ON m.transfer=s.source
LEFT JOIN errors f ON f.transfer=t.id
LEFT JOIN decisions d ON d.root=t.id
ORDER BY t.id DESC
