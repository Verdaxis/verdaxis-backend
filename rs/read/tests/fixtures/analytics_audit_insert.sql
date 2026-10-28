-- Test fixture: one audit row in the disposable analytics schema (Task 19).
INSERT INTO audit_logs(id,user_id,action,resource_type,resource_id,changes,ip_address,request_id,timestamp) VALUES($1,$3,$2,'user',NULL,NULL,NULL,NULL,$4)
