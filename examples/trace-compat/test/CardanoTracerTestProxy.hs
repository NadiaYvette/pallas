{-# LANGUAGE NumericUnderscores #-}
{-# LANGUAGE RecordWildCards #-}
{-# LANGUAGE ScopedTypeVariables #-}
{-# LANGUAGE OverloadedStrings #-}
{-# LANGUAGE LambdaCase #-}

module Main where

import           Cardano.Logging
import qualified Cardano.Logging.Types as Net
import           Cardano.Tracer.Test.ForwardingStressTest.Script
import           Cardano.Tracer.Test.ForwardingStressTest.Types
import           Cardano.Tracer.Test.Utils
import           Ouroboros.Network.Magic (NetworkMagic (..))
import           Ouroboros.Network.NodeToClient (withIOManager)

import           Control.Concurrent (threadDelay)
import           Control.Exception
import           Control.Monad (when)
import           Data.IORef (IORef, newIORef, readIORef, writeIORef)
import qualified Data.List as L
import           Data.Maybe (fromMaybe)
import           Data.Monoid
import           Data.Vector (Vector)
import qualified Data.Vector as Vector
import qualified System.Directory as Sys
import           System.Environment (lookupEnv, setEnv, unsetEnv)
import qualified System.IO as Sys
import           System.PosixCompat.Files (fileExist)
import qualified System.Process as Sys

import           Test.Tasty
import           Test.Tasty.QuickCheck

import           Trace.Forward.Forwarding (InitForwardingConfig (..), initForwarding)
import           Trace.Forward.Utils.TraceObject (writeToSink)
-- import           Trace.Forward.Utils.DataPoint (DataPointStore, writeToStore, DataPoint(..))
-- import           Cardano.Logging.Tracer.EKG (ekgTracer)
import qualified System.Metrics as EKG
import qualified System.Metrics.Gauge as Gauge
import           Data.Aeson (decode, Value(..), Object)
import qualified Data.Aeson.KeyMap as KeyMap
import qualified Data.ByteString.Lazy as BSL
import qualified Data.ByteString.Lazy.Char8 as BS8

main :: IO ()
main = do
    setEnv "TASTY_NUM_THREADS" "1"
    mbWorkdir <- lookupEnv "WORKDIR"

    ts' <- getTestSetup
             TestSetup
             { tsTime         = Last $ Just 10.0
             , tsThreads      = Last $ Just 5
             , tsMessages     = Last   Nothing
             , tsSockInternal = Last $ Just "tracer.sock"
             , tsSockExternal = Last $ Just "tracer.sock"
             , tsNetworkMagic = Last $ Just $ NetworkMagic 42
             , tsWorkDir      = Last $ Just $ fromMaybe "/tmp/testTracerProxy" mbWorkdir
             }

    projectRoot <- Sys.getCurrentDirectory
    tracerRoot <- Sys.canonicalizePath $ unI (tsWorkDir ts')
    exists <- fileExist tracerRoot
    when exists do
      Sys.removeDirectoryRecursive           tracerRoot
    Sys.createDirectoryIfMissing True       (tracerRoot <> "/logs")
    Sys.setCurrentDirectory                  tracerRoot

    let ts = ts' { tsWorkDir = Identity tracerRoot }

    msgCountersRef <- newIORef []
    msgsRef        <- newIORef Vector.empty
    tracerRef      <- newIORef Nothing
    
    let tracerGetter = getProxyTracerState ts tracerRef projectRoot
    
    defaultMain (proxyTests ts msgCountersRef msgsRef tracerGetter)
        `catch` (\ (e :: SomeException) -> do
            unsetEnv "TASTY_NUM_THREADS"
            trState <- readIORef tracerRef
            case trState of
              Nothing -> pure ()
              Just (tracerHdl, proxyHdl, _, _, _) -> do
                Sys.cleanupProcess (Nothing, Nothing, Nothing, tracerHdl)
                Sys.cleanupProcess (Nothing, Nothing, Nothing, proxyHdl)
            throwIO e)

proxyTests :: TestSetup Identity 
            -> IORef [Int] 
            -> IORef (Vector Message) 
            -> IO (Sys.ProcessHandle, Sys.ProcessHandle, Trace IO Message, EKG.Store, DataPointStore) 
            -> TestTree
proxyTests ts msgCountersRef msgsRef tracerGetter =
    testGroup "Pallas Proxy Tests"
    [ localOption (QuickCheckTests 1) $ testGroup "trace-proxy"
        [ testProperty "TraceObjects + EKG + Datapoints via Proxy" $
            runProxyTest ts msgCountersRef msgsRef tracerGetter
        ]
    ]

getProxyTracerState :: TestSetup Identity
                     -> IORef (Maybe (Sys.ProcessHandle, Sys.ProcessHandle, Trace IO Message, EKG.Store, DataPointStore))
                     -> FilePath
                     -> IO (Sys.ProcessHandle, Sys.ProcessHandle, Trace IO Message, EKG.Store, DataPointStore)
getProxyTracerState TestSetup{..} ref projectRoot = do
  state <- readIORef ref
  case state of
    Just st -> pure st
    Nothing -> do
      stdTr <- standardTracer
      
      -- Create EKG Store
      ekgStore <- EKG.newStore
      
      (tracerHdl, proxyHdl, fwdTr, dpStore) <- setupProxyTracer ekgStore
      
      ekgTr <- ekgTracer simpleTestConfig ekgStore
      
      tr <- mkCardanoTracer stdTr fwdTr (Just ekgTr) ["Test"]
      let st = (tracerHdl, proxyHdl, tr, ekgStore, dpStore)
      writeIORef ref $ Just st
      pure st
 where
   setupProxyTracer ekgStore = do
     let tracerRealSock = unI tsWorkDir <> "/tracer-real.sock"
     let nodeSock = unI tsSockExternal -- "tracer.sock"

     -- 1. Start trace-compat (Tracer) listening on tracer-real.sock
     Sys.writeFile "config.yaml" . L.unlines $
       [ "networkMagic: " <> show (unNetworkMagic $ unI tsNetworkMagic)
       , "network:"
       , "  tag: AcceptAt"
       , "  contents: \""<> tracerRealSock <>"\""
       , "logging:"
       , "- logRoot: \"" <> unI tsWorkDir <> "/logs\""
       , "  logMode: FileMode"
       , "  logFormat: ForMachine"
       ]
     
     let cp = (Sys.proc "cabal" [ "run", "exe:cardano-tracer", "--", "--config", unI tsWorkDir <> "/config.yaml"]) { Sys.cwd = Just projectRoot }
     (_, _, _, tracerHdl) <- Sys.createProcess cp
     threadDelay 5_000_000
     resTracer <- Sys.getProcessExitCode tracerHdl
     case resTracer of
       Nothing   -> putStrLn "trace-compat (real tracer) started.."
       Just code -> error $ "trace-compat failed to start with code " <> show code

     -- 2. Start trace-proxy listening on nodeSock and connecting to tracerRealSock
     proxyHdl <- Sys.spawnProcess "/home/nyc/src/pallas/target/release/trace-proxy"
       [ "--node-socket", nodeSock
       , "--tracer-socket", tracerRealSock
       , "--magic", show (unNetworkMagic $ unI tsNetworkMagic)
       ]
     threadDelay 1_000_000
     resProxy <- Sys.getProcessExitCode proxyHdl
     case resProxy of
       Nothing   -> putStrLn "trace-proxy started.."
       Just code -> error $ "trace-proxy failed to start with code " <> show code

     -- 3. Initialize Forwarding (Node) connecting to nodeSock (proxy)
     (forwardSink, dpStore) <- withIOManager \iomgr -> do
       let forwardingConf = fromMaybe defaultForwarder (tcForwarder simpleTestConfig)
       initForwarding iomgr forwardingConf{ tofQueueSize = 1000 } $
         InitForwardingWith
           { initNetworkMagic          = unI tsNetworkMagic
           , initEKGStore              = Just ekgStore
           , initHowToConnect          = Net.LocalPipe nodeSock
           , initForwarderMode         = Initiator
           , initOnForwardInterruption = Nothing
           , initOnQueueOverflow       = Nothing
           }
     pure (tracerHdl, proxyHdl, forwardTracer (writeToSink forwardSink), dpStore)

runProxyTest :: TestSetup Identity
              -> IORef [Int]
              -> IORef (Vector Message)
              -> IO (Sys.ProcessHandle, Sys.ProcessHandle, Trace IO Message, EKG.Store, DataPointStore)
              -> Property
runProxyTest TestSetup{..} _msgCountersRef _msgsRef tracerGetter = ioProperty do
    (_, _, tr, ekgStore, dpStore) <- tracerGetter
    
    -- 1. EKG Test
    g <- EKG.createGauge "test.gauge" ekgStore
    Gauge.set g 123
    
    -- 2. Datapoint Test
    writeToStore dpStore "test.datapoint" (DataPoint (String "datapoint-value"))

    -- 3. TraceObject Test
    let msg = Message1 42 100
    traceWith tr msg

    -- 4. Wait for forwarding and polling
    threadDelay 5_000_000 

    -- 5. Verify
    let logsRootDir = unI tsWorkDir <> "/logs"
    
    -- Wait for ANY subdirectory to appear in logsRootDir
    let waitForSubdir path retries = do
          exists <- Sys.doesDirectoryExist path
          if exists 
            then do
              contents <- Sys.listDirectory path
              case contents of
                [] -> if retries > 0 then threadDelay 1_000_000 >> waitForSubdir path (retries - 1) else return Nothing
                (d:_) -> return (Just d)
            else if retries > 0 
                   then threadDelay 1_000_000 >> waitForSubdir path (retries - 1)
                   else return Nothing

    mbSubDir <- waitForSubdir logsRootDir (60 :: Int)
    subDir <- case mbSubDir of
                Just d -> pure d
                Nothing -> fail "No subdirectory found in logs"
    
    let logsDir = logsRootDir <> "/" <> subDir
    
    -- Wait for file existence
    let waitForFile path retries = do
          exists <- fileExist path
          if exists 
            then return True
            else if retries > 0 
                   then threadDelay 1_000_000 >> waitForFile path (retries - 1)
                   else return False

    -- Increase retries to 60 (60 seconds)
    _ <- waitForFile (logsDir <> "/ekg.json") (60 :: Int)
    _ <- waitForFile (logsDir <> "/datapoints.json") (60 :: Int)

    -- Check EKG
    ekgContent <- BSL.readFile (logsDir <> "/ekg.json")
    let ekgLines = BS8.lines ekgContent
    
    let checkEkg name val = 
          any (\line -> case decode line of
                          Just (obj :: Object) -> 
                            case KeyMap.lookup "name" obj of
                              Just (String n) | n == name -> 
                                case KeyMap.lookup "value" obj of
                                  Just valObj -> 
                                     case valObj of
                                       Object v -> 
                                         case KeyMap.lookup "val" v of
                                            Just (Number x) -> show x == show val
                                            _ -> False
                                       _ -> False
                                  _ -> False
                              _ -> False
                          _ -> False
              ) ekgLines

    let ekgPass = checkEkg "test.gauge" (123.0 :: Double)
    
    -- Check Datapoints
    dpContent <- BSL.readFile (logsDir <> "/datapoints.json")
    let dpLines = BS8.lines dpContent
    
    let checkDp name =
          any (\line -> case decode line of
                          Just (obj :: Object) -> 
                            case KeyMap.lookup "name" obj of
                              Just (String n) | n == name -> True
                              _ -> False
                          _ -> False
              ) dpLines
              
    let dpPass = checkDp "test.datapoint"
    
    if ekgPass && dpPass 
      then pure $ property True
      else do
        putStrLn $ "EKG Pass: " <> show ekgPass
        putStrLn $ "DP Pass: " <> show dpPass
        pure $ property False
